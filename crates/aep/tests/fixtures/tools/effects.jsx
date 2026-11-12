// effects.jsx - builds the combined EFFECTS fixture project (ExtendScript, ES3).
//
// One composition per case of the synthetic corpus (MANIFEST.md, effects family), named exactly
// like the case's single-change fixture (e.g. "216-fx-gaussianblur-40"). Every case comp is a
// duplicate of the base comp "000-base" (comp C of 000-base: 1920x1080, 25 fps, 5 s, black,
// one red 1920x1080 solid layer "L") plus that case's change (and its FROM chain), so the
// importer's result for the old fixture's comp C must equal the case comp here.
// The "-on-M" cases start from "520-solid-M-grey-960x540" (base + a grey 960x540 solid "M" on
// top). Case 905 is built from its own recipe (steps.png footage, 24 effect layers).
//
// Order rule: the FIRST instance of an effect type in the project carries the parameter
// definitions (parT/pard) that later instances borrow, and AE keeps some set values in that
// pard; so for every effect type the case that adds it at its defaults comes first (214 before
// 216, 368 before 369, 712 before 713-723 and 905, ...).
//
// Deterministic and idempotent: it opens the empty project EMPTY_AEP (closing the current
// project WITHOUT saving), builds everything, and saves to OUT_AEP.
//
// The same CASES table drives the MCP runner (to-mcp.js turns it into batch.run operations);
// keep both in step by changing only the table.
//
// Step vocabulary (layer = layer name in the case comp, fx = 1-based effect index):
//   ["fx", layer, matchName]            add an effect (optional 4th: instance name)
//   ["set", layer, fx, prop, value]     set a parameter (prop = display name or match name;
//                                       points in layer pixels, colours 0..1, layers by index)
//   ["key", layer, fx, prop, t, value]  keyframe at layer time t (s), AE's scripted default
//   ["expr", layer, fxMatch, prop, code] expression on a parameter of the effect with that match name
//   ["eval", layer, fxMatch, prop, t]   read the value at time t (AE writes the key speeds of a
//                                       keyed multi-dimensional parameter only after evaluating
//                                       it; without this 364/382 differ in the first key's speed)
//   ["move", layer, fx, to]             move effect fx to stack position `to`
//   ["items", layer, fx, [..]]          Dropdown Menu Control items
//   ["solid", name, [r,g,b], w, h]      new solid layer on top
//   ["null", name]                      new null layer on top
//   ["footage", name]                   new layer of the shared footage item STEPS on top
//   ["delete", layer]                   delete a layer
//   ["video", layer, bool]              video switch

var EMPTY_AEP = "<corpus>/combined/gen/empty-work.aep"; // a copy of synthetic/001-empty.aep
var OUT_AEP = "<corpus>/combined/effects.aep";
var STEPS_PNG = "<corpus>/synthetic/test-media/steps.png";

// ---------------------------------------------------------------- the case table
function fx(layer, mn) { return ["fx", layer, mn]; }
function set1(layer, prop, v) { return ["set", layer, 1, prop, v]; }
function key1(layer, prop, t, v) { return ["key", layer, 1, prop, t, v]; }
function cat(a, b) { return a.concat(b); }

var SLIDER = [fx("L", "ADBE Slider Control")];
var GAUSS = [fx("L", "ADBE Gaussian Blur 2")];
var FILL = [fx("L", "ADBE Fill")];
var LAYERCTL = [fx("L", "ADBE Layer Control"), set1("L", "Layer", 1)];
var DROPDOWN = [fx("L", "ADBE Dropdown Control"), ["items", "L", 1, ["Alpha", "Beta", "Gamma", "Delta", "Epsilon"]]];
var TINT = [fx("L", "ADBE Tint")];
var RAMP = [fx("L", "ADBE Ramp")];
var EXPO = [fx("L", "ADBE Exposure2")];
var INVERT = [fx("L", "ADBE Invert")];
var BOX = [fx("L", "ADBE Box Blur2")];
var SHADOW = [fx("L", "ADBE Drop Shadow")];
var GLOW = [fx("L", "ADBE Glo2")];
var HUESAT = [fx("L", "ADBE HUE SATURATION")];
var LEVELS = [fx("L", "ADBE Easy Levels2")];
var TIMEWARP = [fx("L", "ADBE Timewarp")];

// base: "000-base" (L only) or "520" (M on top of L)
var CASES = [
    { id: "071-fx-slider-value77", steps: cat(SLIDER, [set1("L", "Slider", 77)]) },
    { id: "073-fx-color-0.2-0.6-1", steps: [fx("L", "ADBE Color Control"), set1("L", "Color", [0.2, 0.6, 1])] },
    { id: "077-fx-dropdown-default", steps: [fx("L", "ADBE Dropdown Control")] },
    { id: "080-fx-dropshadow-default", steps: SHADOW },
    { id: "214-fx-gaussianblur-default", steps: GAUSS },
    { id: "215-fx-fill-default", steps: FILL },
    { id: "216-fx-gaussianblur-40", steps: cat(GAUSS, [set1("L", "Blurriness", 40)]) },
    { id: "217-fx-checkbox-on", steps: [fx("L", "ADBE Checkbox Control"), set1("L", "Checkbox", 1)] },
    { id: "218-fx-point-111-222", steps: [fx("L", "ADBE Point Control"), set1("L", "Point", [111, 222])] },
    { id: "219-fx-angle-90", steps: [fx("L", "ADBE Angle Control"), set1("L", "Angle", 90)] },
    { id: "220-fx-layercontrol-to-layer1", steps: [["null", "Null"], fx("L", "ADBE Layer Control"), set1("L", "Layer", 1)] },
    { id: "221-fx-slider-2key", steps: cat(SLIDER, [key1("L", "Slider", 0, 10), key1("L", "Slider", 2, 90)]) },
    { id: "300-fx-gaussianblur-0-explicit", steps: cat(GAUSS, [set1("L", "Blurriness", 0)]) },
    { id: "301-fx-gaussianblur-40-then-25", steps: cat(GAUSS, [set1("L", "Blurriness", 40), set1("L", "Blurriness", 25)]) },
    { id: "302-fx-fill-opacity-100-explicit", steps: cat(FILL, [set1("L", "Opacity", 1)]) },
    { id: "303-fx-fill-opacity-50-then-100", steps: cat(FILL, [set1("L", "Opacity", 0.5), set1("L", "Opacity", 1)]) },
    { id: "304-fx-slider-0-explicit", steps: cat(SLIDER, [set1("L", "Slider", 0)]) },
    { id: "320-fx-layercontrol-self-explicit", steps: LAYERCTL },
    { id: "321-fx-layercontrol-none", steps: cat(LAYERCTL, [set1("L", "Layer", 0)]) },
    { id: "323-fx-dropdown-5items", steps: DROPDOWN },
    { id: "324-fx-dropdown-5items-select3", steps: cat(DROPDOWN, [set1("L", "Menu", 3)]) },
    { id: "356-fx-tint-default", steps: TINT },
    { id: "357-fx-tint-black-0.2-0.4-0.6", steps: cat(TINT, [set1("L", "Map Black To", [0.2, 0.4, 0.6])]) },
    { id: "361-fx-tint-amount-0", steps: cat(TINT, [set1("L", "Amount to Tint", 0)]) },
    { id: "363-fx-tint-amount-2key", steps: cat(TINT, [key1("L", "Amount to Tint", 0, 20), key1("L", "Amount to Tint", 2, 80)]) },
    { id: "364-fx-tint-white-2key", steps: cat(TINT, [key1("L", "Map White To", 0, [1, 1, 0]), key1("L", "Map White To", 2, [0, 0, 1]), ["eval", "L", "ADBE Tint", "Map White To", 1]]) },
    { id: "366-fx-blur-then-tint", steps: cat(TINT, [fx("L", "ADBE Gaussian Blur 2"), ["move", "L", 2, 1]]) },
    { id: "368-fx-ramp-default", steps: RAMP },
    { id: "369-fx-ramp-start-100-200", steps: cat(RAMP, [set1("L", "Start of Ramp", [100, 200])]) },
    { id: "371-fx-ramp-startcolor-red", steps: cat(RAMP, [set1("L", "Start Color", [1, 0, 0])]) },
    { id: "374-fx-ramp-end-2000-m100", steps: cat(RAMP, [set1("L", "End of Ramp", [2000, -100])]) },
    { id: "377-fx-ramp-shape-radial", steps: cat(RAMP, [set1("L", "Ramp Shape", 2)]) },
    { id: "378-fx-ramp-scatter-12", steps: cat(RAMP, [set1("L", "Ramp Scatter", 12)]) },
    { id: "380-fx-ramp-blend-25pct", steps: cat(RAMP, [set1("L", "Blend With Original", 0.25)]) },
    { id: "382-fx-ramp-start-2key", steps: cat(RAMP, [key1("L", "Start of Ramp", 0, [100, 100]), key1("L", "Start of Ramp", 2, [1800, 900]), ["eval", "L", "ADBE Ramp", "Start of Ramp", 1]]) },
    { id: "383-fx-ramp-on-100x100-start-10-20", steps: cat(RAMP, [["solid", "S100", [1, 1, 1], 100, 100], fx("S100", "ADBE Ramp"), set1("S100", "Start of Ramp", [10, 20])]) },
    { id: "389-fx-exposure-default", steps: EXPO },
    { id: "390-fx-exposure-channels-individual", steps: cat(EXPO, [set1("L", "ADBE Exposure2-0001", 2)]) },
    { id: "391-fx-exposure-master-1.5", steps: cat(EXPO, [set1("L", "ADBE Exposure2-0003", 1.5)]) },
    { id: "393-fx-exposure-offset-0.1", steps: cat(EXPO, [set1("L", "ADBE Exposure2-0004", 0.1)]) },
    { id: "394-fx-exposure-gamma-2.2", steps: cat(EXPO, [set1("L", "ADBE Exposure2-0005", 2.2)]) },
    { id: "395-fx-exposure-individual-red-1", steps: cat(EXPO, [set1("L", "ADBE Exposure2-0001", 2), set1("L", "ADBE Exposure2-0008", 1)]) },
    { id: "396-fx-exposure-bypass-on", steps: cat(EXPO, [set1("L", "ADBE Exposure2-0022", 1)]) },
    { id: "397-fx-exposure-master-2key", steps: cat(EXPO, [key1("L", "ADBE Exposure2-0003", 0, -1), key1("L", "ADBE Exposure2-0003", 2, 2)]) },
    { id: "410-fx-invert-channel-2", steps: cat(INVERT, [set1("L", "Channel", 2)]) },
    { id: "411-fx-invert-blend-50", steps: cat(INVERT, [set1("L", "Blend With Original", 50)]) },
    { id: "413-fx-fastbox-radius-20", steps: cat(BOX, [set1("L", "Blur Radius", 20)]) },
    { id: "414-fx-fastbox-iterations-1", steps: cat(BOX, [set1("L", "Iterations", 1)]) },
    { id: "416-fx-fastbox-repeatedge-off", steps: cat(BOX, [set1("L", "ADBE Box Blur2-0004", 0)]) },
    { id: "418-fx-gaussian-dimensions-3", steps: cat(GAUSS, [set1("L", "Blur Dimensions", 3)]) },
    { id: "419-fx-gaussian-repeatedge-off", steps: cat(GAUSS, [set1("L", "ADBE Gaussian Blur 2-0003", 0)]) },
    { id: "422-fx-dropshadow-opacity-204", steps: cat(SHADOW, [set1("L", "Opacity", 204)]) },
    { id: "426-fx-dropshadow-shadowonly-on", steps: cat(SHADOW, [set1("L", "Shadow Only", 1)]) },
    { id: "427-fx-glow-default", steps: GLOW },
    { id: "429-fx-glow-threshold-76.5", steps: cat(GLOW, [set1("L", "Glow Threshold", 76.5)]) },
    { id: "438-fx-glow-midpoint-0.25", steps: cat(GLOW, [set1("L", "A & B Midpoint", 0.25)]) },
    { id: "444-fx-huesat-hue-90", steps: cat(HUESAT, [set1("L", "ADBE HUE SATURATION-0004", 90)]) },
    { id: "446-fx-huesat-light-m30", steps: cat(HUESAT, [set1("L", "ADBE HUE SATURATION-0006", -30)]) },
    { id: "448-fx-huesat-colorizehue-120", steps: cat(HUESAT, [set1("L", "ADBE HUE SATURATION-0008", 120)]) },
    { id: "512-fx-levels-inblack-0.1", steps: cat(LEVELS, [set1("L", "Input Black", 0.1)]) },
    { id: "514-fx-levels-gamma-2", steps: cat(LEVELS, [set1("L", "Gamma", 2)]) },
    { id: "516-fx-levels-outwhite-0.9", steps: cat(LEVELS, [set1("L", "Output White", 0.9)]) },
    { id: "529-fx-timewarp-default", steps: TIMEWARP },
    { id: "530-fx-timewarp-speed-25", steps: cat(TIMEWARP, [set1("L", "Speed", 25)]) },
    { id: "531-fx-timewarp-speed-keyed-50-200", steps: cat(TIMEWARP, [key1("L", "Speed", 0, 50), key1("L", "Speed", 2, 200)]) },
    { id: "532-fx-timewarp-method-1", steps: cat(TIMEWARP, [set1("L", "Method", 1)]) },
    { id: "533-fx-timewarp-sourceframe-12", steps: cat(TIMEWARP, [set1("L", "Adjust Time By", 2), set1("L", "Source Frame", 12)]) },
    { id: "534-fx-slider-expression-time10", steps: cat(SLIDER, [["expr", "L", "ADBE Slider Control", "Slider", "time*10"]]) },
    { id: "610-fx-tint-named-My-Tint", steps: [["fx", "L", "ADBE Tint", "My Tint"]] },
    // ---- from 520: grey 960x540 solid M on top of L
    { id: "711-fx-directional-blur-30-40-on-M", base: "520", steps: [fx("M", "ADBE Motion Blur"), set1("M", "Direction", 30), set1("M", "Blur Length", 40)] },
    { id: "712-fx-eleven-effects-defaults-on-M", base: "520", steps: [fx("M", "ADBE Radial Blur"), fx("M", "CC Radial Blur"), fx("M", "ADBE Venetian Blinds"), fx("M", "ADBE Channel Blur"), fx("M", "ADBE Black&White"), fx("M", "ADBE Tritone"), fx("M", "ADBE Luma Key"), fx("M", "ADBE Simple Choker"), fx("M", "ADBE Matte Choker"), fx("M", "ADBE Geometry2"), fx("M", "ADBE Set Matte3")] },
    { id: "713-fx-radial-blur-zoom-20-on-M", base: "520", steps: [fx("M", "ADBE Radial Blur"), set1("M", "Amount", 20), set1("M", "Center", [600, 300]), set1("M", "Type", 2), set1("M", "Antialiasing (Best Quality)", 2)] },
    { id: "714-fx-ccradial-type2-30-60-on-M", base: "520", steps: [fx("M", "CC Radial Blur"), set1("M", "Type", 2), set1("M", "Amount", 30), set1("M", "Quality", 60), set1("M", "Center", [400, 200])] },
    { id: "715-fx-venetian-40-30-50-5-on-M", base: "520", steps: [fx("M", "ADBE Venetian Blinds"), set1("M", "Transition Completion", 40), set1("M", "Direction", 30), set1("M", "Width", 50), set1("M", "Feather", 5)] },
    { id: "716-fx-channel-blur-5-10-15-20-edge-horiz-on-M", base: "520", steps: [fx("M", "ADBE Channel Blur"), set1("M", "Red Blurriness", 5), set1("M", "Green Blurriness", 10), set1("M", "Blue Blurriness", 15), set1("M", "Alpha Blurriness", 20), set1("M", "ADBE Channel Blur-0005", 1), set1("M", "Blur Dimensions", 2)] },
    { id: "717-fx-blackwhite-set-tint-on-L", base: "520", steps: [fx("L", "ADBE Black&White"), set1("L", "Reds", 10), set1("L", "Yellows", 70), set1("L", "Greens", 30), set1("L", "Cyans", 50), set1("L", "Blues", 90), set1("L", "Magentas", 20), set1("L", "ADBE Black&White-0007", 1), set1("L", "Tint Color", [1, 0.5, 0])] },
    { id: "718-fx-tritone-set-blend25-on-L", base: "520", steps: [fx("L", "ADBE Tritone"), set1("L", "Highlights", [1, 1, 0.8]), set1("L", "Midtones", [0.6, 0.2, 0.2]), set1("L", "Shadows", [0, 0, 0.3]), set1("L", "Blend With Original", 25)] },
    { id: "719-fx-lumakey-darker-140-20-1-2.5-on-M", base: "520", steps: [fx("M", "ADBE Luma Key"), set1("M", "ADBE Luma Key-0001", 2), set1("M", "Threshold", 140), set1("M", "Tolerance", 20), set1("M", "Edge Thin", 1), set1("M", "Edge Feather", 2.5)] },
    { id: "720-fx-simplechoker-3.5-on-M", base: "520", steps: [fx("M", "ADBE Simple Choker"), set1("M", "Choke Matte", 3.5)] },
    { id: "721-fx-simplechoker-3.5-view-matte-on-M", base: "520", steps: [fx("M", "ADBE Simple Choker"), set1("M", "Choke Matte", 3.5), set1("M", "View", 2)] },
    { id: "722-fx-mattechoker-set-on-M", base: "520", steps: [fx("M", "ADBE Matte Choker"), set1("M", "Geometric Softness 1", 6), set1("M", "Choke 1", 50), set1("M", "Geometric Softness 2", 2), set1("M", "Choke 2", -10), set1("M", "Iterations", 3)] },
    { id: "723-fx-transform-set-on-M", base: "520", steps: [fx("M", "ADBE Geometry2"), set1("M", "Anchor Point", [400, 200]), set1("M", "Position", [500, 300]), set1("M", "ADBE Geometry2-0011", 0), set1("M", "Scale Height", 80), set1("M", "Scale Width", 120), set1("M", "Skew", 10), set1("M", "Skew Axis", 20), set1("M", "Rotation", 30), set1("M", "Opacity", 70), set1("M", "ADBE Geometry2-0009", 0), set1("M", "Shutter Angle", 90), set1("M", "Sampling", 2)] }
];


// Effect cases of MANIFEST.md that never became PR fixtures (old files in <corpus>/synthetic).
var MORE = [
    { id: "070-fx-slider-default", steps: SLIDER },
    { id: "072-fx-checkbox-default", steps: [fx("L", "ADBE Checkbox Control")] },
    { id: "074-fx-point-default", steps: [fx("L", "ADBE Point Control")] },
    { id: "075-fx-angle-default", steps: [fx("L", "ADBE Angle Control")] },
    { id: "076-fx-layercontrol-default", steps: [fx("L", "ADBE Layer Control")] },
    { id: "078-fx-gaussianblur-25", steps: cat(GAUSS, [set1("L", "Blurriness", 25)]) },
    { id: "079-fx-fill-color-0-0.5-1", steps: cat(FILL, [set1("L", "Color", [0, 0.5, 1])]) },
    { id: "322-fx-layercontrol-none-then-self", steps: cat(LAYERCTL, [set1("L", "Layer", 0), set1("L", "Layer", 1)]) },
    { id: "358-fx-tint-black-red", steps: cat(TINT, [set1("L", "Map Black To", [1, 0, 0])]) },
    { id: "359-fx-tint-white-green", steps: cat(TINT, [set1("L", "Map White To", [0, 1, 0])]) },
    { id: "360-fx-tint-white-0.5-0.25-1", steps: cat(TINT, [set1("L", "Map White To", [0.5, 0.25, 1])]) },
    { id: "362-fx-tint-amount-37.5", steps: cat(TINT, [set1("L", "Amount to Tint", 37.5)]) },
    { id: "365-fx-tint-then-blur", steps: cat(TINT, [fx("L", "ADBE Gaussian Blur 2")]) },
    { id: "370-fx-ramp-start-1500-900", steps: cat(RAMP, [set1("L", "Start of Ramp", [1500, 900])]) },
    { id: "372-fx-ramp-startcolor-0.25-0.5-0.75", steps: cat(RAMP, [set1("L", "Start Color", [0.25, 0.5, 0.75])]) },
    { id: "373-fx-ramp-end-300-700", steps: cat(RAMP, [set1("L", "End of Ramp", [300, 700])]) },
    { id: "375-fx-ramp-endcolor-blue", steps: cat(RAMP, [set1("L", "End Color", [0, 0, 1])]) },
    { id: "376-fx-ramp-endcolor-0.1-0.9-0.3", steps: cat(RAMP, [set1("L", "End Color", [0.1, 0.9, 0.3])]) },
    { id: "379-fx-ramp-scatter-250.5", steps: cat(RAMP, [set1("L", "Ramp Scatter", 250.5)]) },
    { id: "381-fx-ramp-blend-100pct", steps: cat(RAMP, [set1("L", "Blend With Original", 1)]) },
    { id: "392-fx-exposure-master-m2.25", steps: cat(EXPO, [set1("L", "ADBE Exposure2-0003", -2.25)]) },
    { id: "409-fx-invert-default", steps: INVERT },
    { id: "412-fx-fastbox-default", steps: BOX },
    { id: "415-fx-fastbox-dimensions-2", steps: cat(BOX, [set1("L", "Blur Dimensions", 2)]) },
    { id: "417-fx-gaussian-default", steps: GAUSS },
    { id: "420-fx-dropshadow-default", steps: SHADOW },
    { id: "421-fx-dropshadow-color-red", steps: cat(SHADOW, [set1("L", "Shadow Color", [1, 0, 0])]) },
    { id: "423-fx-dropshadow-direction-45", steps: cat(SHADOW, [set1("L", "Direction", 45)]) },
    { id: "424-fx-dropshadow-distance-20", steps: cat(SHADOW, [set1("L", "Distance", 20)]) },
    { id: "425-fx-dropshadow-softness-10", steps: cat(SHADOW, [set1("L", "Softness", 10)]) },
    { id: "428-fx-glow-based-1", steps: cat(GLOW, [set1("L", "Glow Based On", 1)]) },
    { id: "430-fx-glow-radius-50", steps: cat(GLOW, [set1("L", "Glow Radius", 50)]) },
    { id: "431-fx-glow-intensity-2.5", steps: cat(GLOW, [set1("L", "Glow Intensity", 2.5)]) },
    { id: "432-fx-glow-composite-1", steps: cat(GLOW, [set1("L", "Composite Original", 1)]) },
    { id: "433-fx-glow-operation-2", steps: cat(GLOW, [set1("L", "Glow Operation", 2)]) },
    { id: "434-fx-glow-colors-2", steps: cat(GLOW, [set1("L", "Glow Colors", 2)]) },
    { id: "435-fx-glow-looping-1", steps: cat(GLOW, [set1("L", "Color Looping", 1)]) },
    { id: "436-fx-glow-loops-3", steps: cat(GLOW, [set1("L", "Color Loops", 3)]) },
    { id: "437-fx-glow-phase-90", steps: cat(GLOW, [set1("L", "Color Phase", 90)]) },
    { id: "439-fx-glow-colora-red", steps: cat(GLOW, [set1("L", "Color A", [1, 0, 0])]) },
    { id: "440-fx-glow-colorb-blue", steps: cat(GLOW, [set1("L", "Color B", [0, 0, 1])]) },
    { id: "441-fx-glow-dimensions-2", steps: cat(GLOW, [set1("L", "Glow Dimensions", 2)]) },
    { id: "443-fx-huesat-default", steps: HUESAT },
    { id: "445-fx-huesat-sat-50", steps: cat(HUESAT, [set1("L", "ADBE HUE SATURATION-0005", 50)]) },
    { id: "447-fx-huesat-colorize-on", steps: cat(HUESAT, [set1("L", "ADBE HUE SATURATION-0007", 1)]) },
    { id: "449-fx-huesat-colorizesat-60", steps: cat(HUESAT, [set1("L", "ADBE HUE SATURATION-0009", 60)]) },
    { id: "450-fx-huesat-colorizelight-20", steps: cat(HUESAT, [set1("L", "ADBE HUE SATURATION-0010", 20)]) },
    { id: "511-fx-levels-default", steps: LEVELS },
    { id: "513-fx-levels-inwhite-0.8", steps: cat(LEVELS, [set1("L", "Input White", 0.8)]) },
    { id: "515-fx-levels-outblack-0.2", steps: cat(LEVELS, [set1("L", "Output Black", 0.2)]) },
    { id: "522-fx-gaussian-default-on-M", base: "520", steps: [fx("M", "ADBE Gaussian Blur 2")] },
    { id: "523-fx-dropshadow-default-on-M", base: "520", steps: [fx("M", "ADBE Drop Shadow")] },
    { id: "524-fx-glow-default-on-M", base: "520", steps: [fx("M", "ADBE Glo2")] },
    { id: "525-fx-fastbox-20-on-M", base: "520", steps: [fx("M", "ADBE Box Blur2"), set1("M", "Blur Radius", 20)] },
    { id: "609-fx-tint-default-ref", steps: TINT },
    { id: "903-fx-33-effects-defaults-on-L", steps: (function () {
        var m = ["ADBE Brightness & Contrast 2", "ADBE Turbulent Displace", "ADBE Fractal Noise", "ADBE Laser", "CC Radial Fast Blur", "ADBE Noise2", "ADBE Pro Levels2", "ADBE Corner Pin", "ADBE Grid", "ADBE Sharpen", "ADBE Linear Color Key2", "ADBE Radial Wipe", "ADBE Remove Color Matting", "CC Bend It", "CC Power Pin", "ADBE 4ColorGradient", "ADBE Change Color", "ADBE Extract", "ADBE Noise HLS2", "ADBE Scatter", "CC Light Sweep", "ADBE Compound Blur", "ADBE Displacement Map", "ADBE Set Matte3", "ADBE OFMotionBlur", "ADBE Stroke", "ADBE Gaussian Blur", "EXtractoR", "CC Glass", "CC Environment", "ADBE CurvesCustom", "ADBE MESH WARP", "ADBE Lumetri"];
        var s = [];
        for (var i = 0; i < m.length; i++) { s.push(fx("L", m[i])); }
        return s;
    })() }
];
// Size budget: the combined file holds only the PR fixtures (partition.txt, family effects).
// Set WITH_MORE = true to rebuild the full pilot (all 139 cases, effects-full.aep).
var WITH_MORE = false;
if (WITH_MORE) { for (var mi = 0; mi < MORE.length; mi++) { CASES.push(MORE[mi]); } }

// Case 905: comp from 000-base, L deleted; R = grey 1920x1080 solid with a default Gradient
// Ramp, video off, at the bottom; E01..E24 = steps.png layers (E24 on top), each with one
// effect. A layer parameter value "R" is R's index at the time of the set.
var E905 = [
    ["E01", "ADBE Brightness & Contrast 2", [["Brightness", 20], ["Contrast", 30]]],
    ["E02", "ADBE Turbulent Displace", [["Amount", 80], ["Size", 150], ["Complexity", 2], ["Evolution", 45], ["Random Seed", 3]]],
    ["E03", "ADBE Fractal Noise", [["Contrast", 150], ["Brightness", -10], ["Complexity", 4], ["Evolution", 90], ["Random Seed", 5], ["Opacity", 80]]],
    ["E04", "ADBE Laser", [["Starting Point", [200, 300]], ["Ending Point", [1700, 800]], ["Length", 0.6], ["Time", 0.4], ["Starting Thickness", 20], ["Ending Thickness", 40], ["Softness", 0.3], ["Inside Color", [1, 1, 0]], ["Outside Color", [0, 0, 1]]]],
    ["E05", "CC Radial Fast Blur", [["Center", [700, 400]], ["Amount", 70], ["Zoom", 2]]],
    ["E06", "ADBE Noise2", [["Amount of Noise", 30]]],
    ["E07", "ADBE Pro Levels2", [["Input Black", 0.1], ["Input White", 0.9], ["Gamma", 1.5], ["Output Black", 0.05], ["Output White", 0.95], ["Red Gamma", 0.8], ["Green Input White", 0.8], ["Blue Output Black", 0.2]]],
    ["E08", "ADBE Corner Pin", [["Upper Left", [100, 50]], ["Upper Right", [1800, 100]], ["Lower Left", [50, 1000]], ["Lower Right", [1850, 1050]]]],
    ["E09", "ADBE Grid", [["Anchor", [100, 100]], ["Width", 100], ["Height", 80], ["Border", 4], ["Color", [0, 1, 0]], ["Opacity", 70]]],
    ["E10", "ADBE Sharpen", [["Sharpen Amount", 50]]],
    ["E11", "ADBE Linear Color Key2", [["Key Color", [0, 0, 1]], ["Matching Tolerance", 20], ["Matching Softness", 15]]],
    ["E12", "ADBE Radial Wipe", [["Transition Completion", 40], ["Start Angle", 30], ["Wipe Center", [800, 500]], ["Wipe", 2], ["Feather", 20]]],
    ["E13", "ADBE Remove Color Matting", [["Background Color", [1, 1, 1]]]],
    ["E14", "CC Bend It", [["Bend", 30], ["Start", [960, 200]], ["End", [960, 900]]]],
    ["E15", "CC Power Pin", [["Top Left", [200, 100]], ["Top Right", [1700, 50]], ["Bottom Left", [100, 1000]], ["Bottom Right", [1800, 950]]]],
    ["E16", "ADBE 4ColorGradient", [["Point 1", [300, 200]], ["Color 1", [1, 0, 0]], ["Point 2", [1600, 250]], ["Color 2", [0, 1, 0]], ["Point 3", [250, 900]], ["Color 3", [0, 0, 1]], ["Point 4", [1700, 850]], ["Color 4", [1, 1, 0]], ["Blend", 50], ["Opacity", 80]]],
    ["E17", "ADBE Change Color", [["Hue Transform", 90], ["Color To Change", [0, 0, 1]], ["Matching Tolerance", 30]]],
    ["E18", "ADBE Extract", [["Black Point", 30], ["White Point", 220], ["Black Softness", 10]]],
    ["E19", "ADBE Noise HLS2", [["Hue", 20], ["Lightness", 10], ["Saturation", 30]]],
    ["E20", "ADBE Scatter", [["Scatter Amount", 20]]],
    ["E21", "CC Light Sweep", [["Center", [800, 500]], ["Direction", 20], ["Width", 80], ["Sweep Intensity", 60], ["Edge Intensity", 100], ["Light Color", [1, 0.5, 0]]]],
    ["E22", "ADBE Compound Blur", [["Blur Layer", "R"], ["Maximum Blur", 10]]],
    ["E23", "ADBE Displacement Map", [["Displacement Map Layer", "R"], ["Max Horizontal Displacement", 20], ["Max Vertical Displacement", 10]]],
    ["E24", "ADBE Set Matte3", [["Take Matte From Layer", "R"], ["Use For Matte", 5]]]
];

function steps905() {
    var s = [["solid", "R", [0.5, 0.5, 0.5], 1920, 1080], fx("R", "ADBE Ramp"), ["video", "R", false], ["delete", "L"]];
    for (var i = 0; i < E905.length; i++) {
        var e = E905[i];
        s.push(["footage", e[0]]);
        s.push(fx(e[0], e[1]));
        for (var j = 0; j < e[2].length; j++) {
            var v = e[2][j][1];
            if (v === "R") { v = i + 2; } // E01..E(i+1) above R
            s.push(set1(e[0], e[2][j][0], v));
        }
    }
    return s;
}
CASES.push({ id: "905-fx-24-effects-set-one-comp", steps: steps905(), footage: true });
// Numeric case order (so the at-defaults case of each effect type comes first, see above).
CASES.sort(function (a, b) { return a.id < b.id ? -1 : (a.id > b.id ? 1 : 0); });

// Template comps (cases of their own): the base, and the base + M of 520.
var BASE = "000-base";
var BASE_M = "520-solid-M-grey-960x540";

// ---------------------------------------------------------------- ExtendScript runner
function findComp(name) {
    for (var i = 1; i <= app.project.numItems; i++) {
        var it = app.project.item(i);
        if (it instanceof CompItem && it.name === name) { return it; }
    }
    return null;
}

function effectParam(eff, prop) {
    var p = null;
    try { p = eff.property(prop); } catch (e) { p = null; }
    if (p === null) { throw new Error("no parameter " + prop + " on " + eff.matchName); }
    return p;
}

function runStep(comp, st, steps) {
    var op = st[0];
    var layer, eff, p;
    if (op === "fx") {
        eff = comp.layer(st[1]).property("ADBE Effect Parade").addProperty(st[2]);
        if (st.length > 3) { eff.name = st[3]; }
    } else if (op === "set" || op === "key" || op === "expr") {
        eff = comp.layer(st[1]).property("ADBE Effect Parade").property(st[2]); // index or match name
        p = effectParam(eff, st[3]);
        if (op === "set") { p.setValue(st[4]); }
        else if (op === "key") { p.setValueAtTime(st[4], st[5]); }
        else { p.expression = st[4]; }
    } else if (op === "eval") {
        comp.layer(st[1]).property("ADBE Effect Parade").property(st[2]).property(st[3]).valueAtTime(st[4], false);
    } else if (op === "move") {
        comp.layer(st[1]).property("ADBE Effect Parade").property(st[2]).moveTo(st[3]);
    } else if (op === "items") {
        eff = comp.layer(st[1]).property("ADBE Effect Parade").property(st[2]);
        eff.property(1).setPropertyParameters(st[3]);
    } else if (op === "solid") {
        layer = comp.layers.addSolid(st[2], st[1], st[3], st[4], 1);
    } else if (op === "null") {
        layer = comp.layers.addNull();
        layer.name = st[1];
    } else if (op === "footage") {
        layer = comp.layers.add(steps);
        layer.name = st[1];
    } else if (op === "delete") {
        comp.layer(st[1]).remove();
    } else if (op === "video") {
        comp.layer(st[1]).enabled = st[2];
    } else {
        throw new Error("unknown step " + op);
    }
}

function main() {
    app.project.close(CloseOptions.DO_NOT_SAVE_CHANGES);
    app.open(new File(EMPTY_AEP));
    var base = app.project.items.addComp(BASE, 1920, 1080, 1, 5, 25);
    base.bgColor = [0, 0, 0];
    base.layers.addSolid([1, 0, 0], "L", 1920, 1080, 1);
    var baseM = base.duplicate();
    baseM.name = BASE_M;
    baseM.layers.addSolid([0.5, 0.5, 0.5], "M", 960, 540, 1);
    var steps = null;
    for (var i = 0; i < CASES.length; i++) {
        var c = CASES[i];
        if (c.footage && steps === null) {
            steps = app.project.importFile(new ImportOptions(new File(STEPS_PNG)));
        }
        var comp = (c.base === "520" ? baseM : base).duplicate();
        comp.name = c.id;
        for (var j = 0; j < c.steps.length; j++) {
            runStep(comp, c.steps[j], steps);
        }
    }
    app.project.save(new File(OUT_AEP));
    return CASES.length + 2;
}

if (typeof EC_DATA_ONLY === "undefined") {
    main();
}
