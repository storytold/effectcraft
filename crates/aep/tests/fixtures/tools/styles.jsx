// styles.jsx - case table of the combined STYLES fixture project (<corpus>/combined/styles.aep),
// which also holds the three preset-made effect cases 963/964/985 that the effects pilot skipped.
//
// One composition per case, named exactly like the case's single-change fixture. Written in MCP
// operation terms (eval.run is disabled on the MCP); mss-to-mcp.js turns it into batch.run
// lists, "$" stands for the case comp.
//
// Layer styles cannot be added through the MCP in an AE session whose Layer menu has not been
// opened in the UI (command.execute 9000-9008 does nothing; property.add of `dropShadow/enabled`
// is refused; property.set on an absent style is refused), and keystrokes / menus are not
// allowed. MCP-only route: the AE-saved style-default originals of the corpus (951 all nine,
// 952 Drop Shadow, 955 Inner Glow, 959 Gradient Overlay, 960 Stroke) are merged into the project
// (project.merge; each lands in its own folder; its comp "C" is renamed "src-NNN"), and every
// case comp is a comp.duplicate of its FROM source plus the case's own changes by property.set /
// keyframe.apply on match-name paths. The preset cases start from the merged presets base 984
// (= 949: solid "B" 600x400 [0.2,0.4,0.8] over "S" = steps.png, L deleted) plus
// layer.apply_preset of the contributor's preset on S. 340 (text) starts from the base comp "000-base".
// After the cases, the six "src-*" comps are deleted (project.delete_item); their folders, solids
// and steps.png items stay (the case comps use them).
//
// 951 is the duplicate of its own merged source (no change on top). Imported originals (not regenerated; their change is a byte patch or a hand edit that no MCP
// operation can make): 988 (style eye
// switched off), 734 (text byte edit).

var FAMILY = "styles";
var OUT_AEP = "<corpus>/combined/styles.aep";
var SYN = "<corpus>/synthetic/";
var PRESETS = "<corpus>/presets/";

function op(o, a) { return { operation: o, args: a }; }
function st(style, prop) { return ["ADBE Layer Styles", style + "/enabled", style + "/" + prop]; }
function bo(prop) { return ["ADBE Layer Styles", "ADBE Blend Options Group", prop]; }
function adv(prop) { return ["ADBE Layer Styles", "ADBE Blend Options Group", "ADBE Adv Blend Group", prop]; }
function set(path, v) { return op("property.set", { comp: "$", layer: "B", property: path, value: v }); }
function S(style, kv) { var r = []; for (var k in kv) { r.push(set(st(style, k), kv[k])); } return r; }
function cat() { var r = []; for (var i = 0; i < arguments.length; i++) { r = r.concat(arguments[i]); } return r; }

// Sources merged into the project: { key, path }; comp "C" of each becomes "src-<key>".
var SOURCES = [
    { key: "951", path: SYN + "951-styles-all-nine-defaults-on-B.aep" },
    { key: "952", path: SYN + "952-style-drop-shadow-defaults-on-B.aep" },
    { key: "955", path: SYN + "955-style-inner-glow-defaults-on-B.aep" },
    { key: "959", path: SYN + "959-style-gradient-overlay-defaults-on-B.aep" },
    { key: "960", path: SYN + "960-style-stroke-defaults-on-B.aep" },
    { key: "984", path: SYN + "984-presets-save-noise.aep" }
];
// Imported originals: merged, comp "C" renamed to the case id.
var MERGED = [
    { id: "988-style-drop-shadow-switched-off", path: SYN + "988-style-drop-shadow-switched-off.aep", why: "style eye switched off: byte patch, not scriptable" },
    { id: "734-text-box-byteedit-style68-1", path: SYN + "734-text-box-byteedit-style68-1.aep", why: "text byte edit, not scriptable" }
];

var ALL977 = cat(
    [set(bo("ADBE Global Angle2"), 45), set(bo("ADBE Global Altitude2"), 40), set(adv("ADBE Layer Fill Opacity2"), 80),
     set(adv("ADBE R Channel Blend"), 0), set(adv("ADBE Blend Interior"), 1), set(adv("ADBE Blend Ranges"), 0)],
    S("dropShadow", { mode2: 1, color: [0, 0, 1], opacity: 60, useGlobalAngle: 1, localLightingAngle: 30, distance: 12, chokeMatte: 10, blur: 8, noise: 5, layerConceals: 0 }),
    S("innerShadow", { mode2: 1, color: [0, 1, 0], opacity: 55, useGlobalAngle: 1, localLightingAngle: 60, distance: 7, chokeMatte: 15, blur: 9, noise: 3 }),
    S("outerGlow", { mode2: 1, opacity: 65, noise: 2, AEColorChoice: 2, color: [1, 0, 0], gradientSmoothness: 80, glowTechnique: 2, chokeMatte: 12, blur: 20, inputRange: 40, shadingNoise: 10 }),
    S("innerGlow", { mode2: 1, opacity: 45, noise: 4, AEColorChoice: 2, color: [0, 1, 0], gradientSmoothness: 70, glowTechnique: 2, innerGlowSource: 2, chokeMatte: 8, blur: 15, inputRange: 30, shadingNoise: 20 }),
    S("bevelEmboss", { bevelStyle: 1, bevelTechnique: 2, strengthRatio: 150, bevelDirection: 2, blur: 10, softness: 3, useGlobalAngle: 1, localLightingAngle: 70, localLightingAltitude: 50, highlightMode: 1, highlightColor: [1, 1, 0], highlightOpacity: 60, shadowMode: 1, shadowColor: [0, 0, 1], shadowOpacity: 40 }),
    S("chromeFX", { mode2: 1, color: [1, 0, 1], opacity: 70, localLightingAngle: 45, distance: 20, blur: 10, invert: 0 }),
    S("solidFill", { mode2: 5, color: [0, 1, 1], opacity: 50 }),
    S("gradientFill", { mode2: 5, opacity: 70, gradientSmoothness: 50, angle: 30, type: 2, reverse: 1, align: 0, scale: 120, offset: [10, 20] }),
    S("frameFX", { mode2: 5, color: [0, 0, 1], size: 6, opacity: 80, style: 2 })
);

// from: "000-base" (template) or a source key ("952", ...).
var CASES = [
    { id: "340-text-range-two-styles", from: "000-base", steps: [op("layer.create_text", { comp: "$", text: "Hello", name: "T" }),
        op("text.set_style_range", { comp: "$", layer: "T", start: 1, end: 3, fillColor: [1, 0, 0] }),
        op("text.set_style_range", { comp: "$", layer: "T", start: 3, end: 5, fillColor: [0, 0, 1], fontSize: 60 })] },
    { id: "951-styles-all-nine-defaults-on-B", from: "951", steps: [] },
    { id: "966-style-drop-shadow-opacity50-distance20", from: "952", steps: S("dropShadow", { opacity: 50, distance: 20 }) },
    { id: "970-style-stroke-size10-color-green", from: "960", steps: S("frameFX", { size: 10, color: [0, 1, 0] }) },
    { id: "974-style-gradient-overlay-angle45-scale150", from: "959", steps: S("gradientFill", { angle: 45, scale: 150 }) },
    { id: "975-style-fill-opacity0-with-drop-shadow", from: "952", steps: [set(adv("ADBE Layer Fill Opacity2"), 0)] },
    { id: "976-style-drop-shadow-opacity-keyed-100-20-linear", from: "952", steps: [op("keyframe.apply", { comp: "$", layer: "B", property: st("dropShadow", "opacity"), keys: [{ time: 0, value: 100, interp: "linear" }, { time: 2, value: 20, interp: "linear" }] })] },
    { id: "977-styles-all-params-changed", from: "951", steps: ALL977 },
    { id: "979-style-inner-glow-source-2", from: "955", steps: S("innerGlow", { innerGlowSource: 2 }) },
    { id: "963-curves-red-mid-down-on-S", from: "984", steps: [op("layer.apply_preset", { comp: "$", layer: "S", path: PRESETS + "curves-red-mid-down.ffx" })] },
    { id: "964-curves-rgb-s-curve-on-S", from: "984", steps: [op("layer.apply_preset", { comp: "$", layer: "S", path: PRESETS + "curves-rgb-two-points.ffx" })] },
    { id: "985-meshwarp-3x3-vertex-moved-on-S", from: "984", steps: [op("layer.apply_preset", { comp: "$", layer: "S", path: PRESETS + "meshwarp-3x3-center-right.ffx" })] }
];
var EXTRA_CASES = ["000-base"];
