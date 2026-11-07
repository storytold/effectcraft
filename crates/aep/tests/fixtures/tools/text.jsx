// text.jsx - builds the combined TEXT fixture project (ExtendScript, ES3), and, run under node,
// translates the same case table into After Effects MCP batch.run operation lists.
//
// One composition per case of the synthetic corpus (MANIFEST.md, text family in partition.txt),
// named exactly like the case's single-change fixture (e.g. "539-text-hscale-0.9"). Every case
// comp is a duplicate of the base comp "000-base" (comp C of 000-base: 1920x1080, 25 fps, 5 s,
// black, one red 1920x1080 solid layer "L") plus that case's steps: a new text layer "T" on top
// of L and the case's change, with its whole FROM chain replayed (e.g. 400 = 110 -> text
// "A" ETX "B" (386) -> "A" CR "B" (387) -> Auto Leading on). Noise references (384, 399, 725)
// are no-ops.
//
// Text layers are created with the panel's default character state (Times New Roman
// TimesNewRomanPSMT, 100 px, white; the default stores the superscript baseline slot, which
// is why the S17 cases set fontBaselineOption normal first).
//
// Byte-edited cases (732, 733: one paragraph slot of 517 changed in the file, not reachable by
// scripting) are built as 517 here and patched afterwards by patch-text.py (same edit as the
// old fixtures: one byte in the comp's text document).
//
// Step vocabulary (the text layer is always "T"):
//   ["text", s, opts]            new text layer T, text s; opts {box: [w,h], boxPos: [x,y]}
//   ["content", s]               Source Text text -> s
//   ["style", {..}]              text.set_style attributes on the whole document
//   ["range", start, end, {..}]  text.set_style_range on characters [start, end)
//   ["reset", scope]             text.reset_style character|paragraph|both
//   ["anim", name, {..}]         text animator with these properties and a Range Selector
//   ["sel", a, match, v]         property match of Range Selector 1 of animator a -> v
//   ["skey", a, match, [[t,v],..], interp]  keys on that selector property
//   ["get", a, match]            read that selector property (as the old case did)
//   ["rmanim", a]                remove animator a
//   ["anchor", [x,y,z]]          T Transform / Anchor Point
//   ["mask", center, size]       rect mask on T (layer space)
//
// Usage under node: node text.jsx <outdir>   (writes batch-00.json = template comp,
// batch-NN.json = cases, smoke.json = a few cases built on batch-00)

var OUT_AEP = "<corpus>/combined/text.aep";
var EMPTY_AEP = "<corpus>/combined/gen/empty-work.aep"; // a copy of synthetic/001-empty.aep

function cat() {
    var r = [];
    for (var i = 0; i < arguments.length; i++) { r = r.concat(arguments[i]); }
    return r;
}
function st(o) { return [["style", o]]; }

var A = [["text", "A", {}]];
var HELLO = [["text", "Hello", {}]];
var BOX_HELLO = [["text", "Hello", { box: [400, 200] }]];
var B517 = [["text", "The quick brown fox jumps over the lazy dog and keeps running far away", { box: [600, 400], boxPos: [-300, -200] }]];
var HHHH = [["text", "HHHH", {}]];
var B538 = cat(HHHH, st({ fontBaselineOption: "normal" }));
var B113 = cat(A, [["content", "Hello"]], st({ fontSize: 60 }), st({ fillColor: [1, 0.5, 0] }));
var B386 = cat(A, [["content", "A\u0003B"]]);
var B579 = cat(B538, [["anim", "Animator 1", { "ADBE Text Position 3D": [0, -50, 0] }]]);
var B592 = cat(B579, [["anim", "Many", {
    "ADBE Text Anchor Point 3D": [3, 4, 0], "ADBE Text Scale 3D": [150, 120, 100], "ADBE Text Skew": 10,
    "ADBE Text Skew Axis": 20, "ADBE Text Rotation": 45, "ADBE Text Opacity": 37, "ADBE Text Fill Color": [0, 1, 0],
    "ADBE Text Stroke Color": [0, 0, 1], "ADBE Text Stroke Width": 3, "ADBE Text Tracking Amount": 20,
    "ADBE Text Line Spacing": [10, 20], "ADBE Text Blur": [5, 6]
}]]);
var B593 = cat(B592, [["skey", "Animator 1", "ADBE Text Percent Start", [[0, 0], [2, 100]], "ease"]]);
var B594 = cat(B593, [["rmanim", "Many"]]);
var B595 = cat(B594, [["rmanim", "Animator 1"]], st({ applyStroke: true, strokeWidth: 5, strokeColor: [0, 0, 1], horizontalScale: 0.9, verticalScale: 1.5, baselineShift: 41 }));
var B727 = cat(B538, [["reset", "paragraph"]]);
function sel(m, v) { return cat(B579, [["sel", "Animator 1", m, v]]); }

var CASES = [
    { id: "110-text-A", steps: A },
    { id: "113-text-Hello-size60-fillorange", steps: B113 },
    { id: "114-text-animator-opacity", steps: cat(B113, [["anim", "Animator 1", { "ADBE Text Opacity": 0 }]]) },
    { id: "243-text-multiline-A-CR-B", steps: [["text", "A\rB", {}]] },
    { id: "245-text-box-400x200-Hello", steps: BOX_HELLO },
    { id: "246-text-box-center-align", steps: cat(BOX_HELLO, st({ justification: "center" })) },
    { id: "247-text-tracking-100", steps: cat(HELLO, st({ tracking: 100 })) },
    { id: "248-text-range-fill-red-1-3", steps: cat(HELLO, [["range", 1, 3, { fillColor: [1, 0, 0] }]]) },
    { id: "334-text-box-right-align", steps: cat(BOX_HELLO, st({ justification: "right" })) },
    { id: "335-text-point-fauxbold", steps: cat(HELLO, st({ fauxBold: true })) },
    { id: "336-text-point-fauxitalic", steps: cat(HELLO, st({ fauxItalic: true })) },
    { id: "337-text-point-allcaps", steps: cat(HELLO, st({ allCaps: true })) },
    { id: "338-text-point-smallcaps", steps: cat(HELLO, st({ smallCaps: true })) },
    { id: "339-text-point-leading60", steps: cat(HELLO, st({ leading: 60 })) },
    { id: "385-text-anchor-10-20", steps: cat(A, [["anchor", [10, 20, 0]]]) },
    { id: "386-text-etx-A-B", steps: B386 },
    { id: "400-text-autoleading-on", steps: cat(B386, [["content", "A\rB"]], st({ autoLeading: true })) },
    { id: "517-text-box-600x400-wrap", steps: B517 },
    { id: "526-text-point-HHHH-default", steps: HHHH },
    { id: "527-text-point-HHHH-baseline-normal", steps: cat(HHHH, st({ fontBaselineOption: "normal" })) },
    { id: "528-text-point-HHHH-baseline-subscript", steps: cat(HHHH, st({ fontBaselineOption: "normal" }), st({ fontBaselineOption: "subscript" })) },
    { id: "538-text-point-HHHH-normal-ref", steps: B538 },
    { id: "539-text-hscale-0.9", steps: cat(B538, st({ horizontalScale: 0.9 })) },
    { id: "540-text-vscale-1.5", steps: cat(B538, st({ verticalScale: 1.5 })) },
    { id: "541-text-baselineshift-41", steps: cat(B538, st({ baselineShift: 41 })) },
    { id: "542-text-autokern-none", steps: cat(B538, st({ autoKernType: "none" })) },
    { id: "543-text-autokern-optical", steps: cat(B538, st({ autoKernType: "optical" })) },
    { id: "544-text-applystroke-on", steps: cat(B538, st({ applyStroke: true })) },
    { id: "545-text-strokewidth-5", steps: cat(B538, st({ strokeWidth: 5 })) },
    { id: "546-text-applyfill-off", steps: cat(B538, st({ applyFill: false })) },
    { id: "548-text-linejoin-round", steps: cat(B538, st({ lineJoinType: "round" })) },
    { id: "550-text-strokeoverfill-off", steps: cat(B538, st({ strokeOverFill: false })) },
    { id: "552-text-nobreak-on", steps: cat(B538, st({ noBreak: true })) },
    { id: "553-text-tsume-0.5", steps: cat(B538, st({ tsume: 0.5 })) },
    { id: "554-text-kerning-50", steps: cat(B538, st({ kerning: 50 })) },
    { id: "555-text-ligature-on", steps: cat(B538, st({ ligature: true })) },
    { id: "556-text-autohyphenate-off", steps: cat(B538, st({ autoHyphenate: false })) },
    { id: "558-text-hangingroman-on", steps: cat(B538, st({ hangingRoman: true })) },
    { id: "559-text-everyline-on", steps: cat(B538, st({ everyLineComposer: true })) },
    { id: "560-text-firstlineindent-20", steps: cat(B538, st({ firstLineIndent: 20 })) },
    { id: "561-text-startindent-30", steps: cat(B538, st({ startIndent: 30 })) },
    { id: "562-text-endindent-40", steps: cat(B538, st({ endIndent: 40 })) },
    { id: "563-text-spacebefore-10", steps: cat(B538, st({ spaceBefore: 10 })) },
    { id: "564-text-spaceafter-12", steps: cat(B538, st({ spaceAfter: 12 })) },
    { id: "565-text-justify-fullleft", steps: cat(B538, st({ justification: "fullLeft" })) },
    { id: "566-text-justify-fullcenter", steps: cat(B538, st({ justification: "fullCenter" })) },
    { id: "567-text-justify-fullright", steps: cat(B538, st({ justification: "fullRight" })) },
    { id: "568-text-justify-fullall", steps: cat(B538, st({ justification: "fullLastLineFull" })) },
    { id: "569-text-direction-rtl", steps: cat(B538, st({ direction: "rightToLeft" })) },
    { id: "571-text-digitset-arabic", steps: cat(B538, st({ digitSet: "arabic" })) },
    { id: "572-text-leadingtype-japanese", steps: cat(B538, st({ leadingType: "japanese" })) },
    { id: "573-text-baselinedir-rotated", steps: cat(B538, st({ baselineDirection: "verticalRotated" })) },
    { id: "575-text-vertical-rtl", steps: cat(B538, st({ lineOrientation: "verticalRtl" })) },
    { id: "576-text-baselinedir-crossstream", steps: cat(B538, st({ baselineDirection: "verticalCrossStream" })) },
    { id: "578-text-digitset-hindi", steps: cat(B538, st({ digitSet: "hindi" })) },
    { id: "579-textanim-position-0-m50", steps: B579 },
    { id: "580-textanim-start-25", steps: sel("ADBE Text Percent Start", 25) },
    { id: "581-textanim-end-75", steps: sel("ADBE Text Percent End", 75) },
    { id: "582-textanim-offset-10", steps: sel("ADBE Text Percent Offset", 10) },
    { id: "583-textanim-units-index", steps: cat(sel("ADBE Text Range Units", 2), [["get", "Animator 1", "ADBE Text Index Start"], ["get", "Animator 1", "ADBE Text Index End"]]) },
    { id: "584-textanim-basedon-3-words", steps: sel("ADBE Text Range Type2", 3) },
    { id: "585-textanim-mode-2-subtract", steps: sel("ADBE Text Selector Mode", 2) },
    { id: "586-textanim-amount-50", steps: sel("ADBE Text Selector Max Amount", 50) },
    { id: "587-textanim-shape-2-rampup", steps: sel("ADBE Text Range Shape", 2) },
    { id: "588-textanim-smoothness-50", steps: sel("ADBE Text Selector Smoothness", 50) },
    { id: "589-textanim-easehigh-30", steps: sel("ADBE Text Levels Max Ease", 30) },
    { id: "590-textanim-easelow-40", steps: sel("ADBE Text Levels Min Ease", 40) },
    { id: "591-textanim-randomize-seed-7", steps: cat(B579, [["sel", "Animator 1", "ADBE Text Randomize Order", 1], ["sel", "Animator 1", "ADBE Text Random Seed", 7]]) },
    { id: "592-textanim-many-props", steps: B592 },
    { id: "593-textanim-start-keyed-ease-0-100", steps: B593 },
    { id: "594-textanim-position-start-keyed-ease", steps: B594 },
    { id: "595-text-stroke-scale-shift-combined", steps: B595 },
    { id: "596-text-combined-fill-over-stroke", steps: cat(B595, st({ strokeOverFill: false })) },
    { id: "705-mask-on-text-rect-40-m70-160-10", steps: cat(B538, [["mask", [100, -30], [120, 80]]]) },
    { id: "725-text-point-HHHH-normal-ref-S18", steps: B538 },
    { id: "727-text-reset-paragraph", steps: B727 },
    { id: "728-textanim-character-offset-3", steps: cat(B727, [["anim", "Off", { "ADBE Text Character Offset": 3 }]]) },
    { id: "732-text-box-byteedit-para24-0", steps: B517, patch: "para24" },
    { id: "733-text-box-byteedit-para35-0", steps: B517, patch: "para35" }
];

var BASE = "000-base";
var TEXT_GROUP = ["ADBE Text Properties", "ADBE Text Animators"];
// Range Selector properties that sit in its "Advanced" group.
var ADVANCED = { "ADBE Text Range Units": 1, "ADBE Text Range Type2": 1, "ADBE Text Selector Mode": 1, "ADBE Text Selector Max Amount": 1,
    "ADBE Text Range Shape": 1, "ADBE Text Selector Smoothness": 1, "ADBE Text Levels Max Ease": 1, "ADBE Text Levels Min Ease": 1,
    "ADBE Text Randomize Order": 1, "ADBE Text Random Seed": 1 };

// ---------------------------------------------------------------- MCP translation (node)
function mcpOps(comp, s) {
    var op = s[0];
    if (op === "text") {
        var a = { comp: comp, text: s[1], name: "T" };
        if (s[2].box) a.boxSize = s[2].box;
        if (s[2].boxPos) a.boxPosition = s[2].boxPos;
        return [{ operation: "layer.create_text", args: a }];
    }
    if (op === "content") return [{ operation: "text.set_content", args: { comp: comp, layer: "T", text: s[1] } }];
    if (op === "style" || op === "range") {
        var o = op === "style" ? s[1] : s[3];
        var b = { comp: comp, layer: "T" };
        for (var k in o) b[k] = o[k];
        if (op === "range") { b.rangeType = "characters"; b.start = s[1]; b.end = s[2]; }
        return [{ operation: op === "style" ? "text.set_style" : "text.set_style_range", args: b }];
    }
    if (op === "reset") return [{ operation: "text.reset_style", args: { comp: comp, layer: "T", scope: s[1] } }];
    if (op === "anim") return [{ operation: "text.add_animator", args: { comp: comp, layer: "T", name: s[1], properties: s[2], selector: "range" } }];
    var selPath = function (a, m) { return TEXT_GROUP.concat([a, "ADBE Text Selectors", "Range Selector 1"], ADVANCED[m] ? ["ADBE Text Range Advanced", m] : [m]); };
    if (op === "get") return [{ operation: "property.get", args: { comp: comp, layer: "T", property: selPath(s[1], s[2]) } }];
    if (op === "sel") return [{ operation: "property.set", args: { comp: comp, layer: "T", property: selPath(s[1], s[2]), value: s[3] } }];
    if (op === "skey") {
        var keys = [];
        for (var i = 0; i < s[3].length; i++) keys.push({ time: s[3][i][0], value: s[3][i][1], interp: s[4] });
        return [{ operation: "keyframe.apply", args: { comp: comp, layer: "T", property: selPath(s[1], s[2]), keys: keys } }];
    }
    if (op === "rmanim") return [{ operation: "property.remove", args: { comp: comp, layer: "T", property: TEXT_GROUP.concat([s[1]]) } }];
    if (op === "anchor") return [{ operation: "property.set", args: { comp: comp, layer: "T", property: ["ADBE Transform Group", "ADBE Anchor Point"], value: s[1] } }];
    if (op === "mask") return [{ operation: "mask.add", args: { comp: comp, layer: "T", shape: "rect", center: s[1], size: s[2] } }];
    throw new Error("unknown step " + op);
}

if (typeof app === "undefined" && typeof require === "function") {
    var fs = require("fs");
    var path = require("path");
    var out = process.argv[2] || path.join(__dirname, "ops-text");
    fs.mkdirSync(out, { recursive: true });
    var templates = [
        { operation: "comp.create", args: { name: BASE, width: 1920, height: 1080, fps: 25, duration: 5, bgColor: [0, 0, 0] } },
        { operation: "layer.create_solid", args: { comp: BASE, name: "L", color: [1, 0, 0], width: 1920, height: 1080 } }
    ];
    fs.writeFileSync(path.join(out, "batch-00.json"), JSON.stringify(templates));
    // Pass 1 creates every case comp and its text layer (the first step of every case is the
    // "text" step), pass 2 runs the remaining steps: a new text layer takes the panel's current
    // character state, so no layer is created after another layer was styled.
    var createOps = function (c) {
        return [{ operation: "comp.duplicate", args: { comp: BASE, newName: c.id } }].concat(mcpOps(c.id, c.steps[0]));
    };
    var changeOps = function (c) {
        var list = [];
        for (var j = 1; j < c.steps.length; j++) list = list.concat(mcpOps(c.id, c.steps[j]));
        return list;
    };
    var write = function (prefix, list, per, fn) {
        var n = 1, cur = [], count = 0;
        for (var ci = 0; ci < list.length; ci++) {
            cur = cur.concat(fn(list[ci]));
            if (++count >= per || ci === list.length - 1) {
                if (cur.length) fs.writeFileSync(path.join(out, prefix + (n < 10 ? "0" : "") + n + ".json"), JSON.stringify(cur));
                n++; cur = []; count = 0;
            }
        }
        return n - 1;
    };
    var nc = write("create-", CASES, 40, createOps);
    var nx = write("change-", CASES, 20, changeOps);
    var SMOKE = ["110-", "113-", "245-", "248-", "385-", "386-", "538-", "554-", "575-", "583-", "591-", "592-", "593-", "595-", "705-", "727-", "728-"];
    var smoke = [];
    for (var q = 0; q < CASES.length; q++) {
        for (var r = 0; r < SMOKE.length; r++) if (CASES[q].id.indexOf(SMOKE[r]) === 0) smoke.push(CASES[q]);
    }
    write("smoke-create", smoke, 100, createOps);
    write("smoke-change", smoke, 100, changeOps);
    console.log("create batches", nc, "change batches", nx, "cases", CASES.length);
}

// ---------------------------------------------------------------- ExtendScript runner
function findText(comp) { return comp.layer("T"); }

function applyAttrs(td, o) {
    var map = { fontSize: "fontSize", fillColor: "fillColor", strokeColor: "strokeColor", strokeWidth: "strokeWidth", tracking: "tracking",
        leading: "leading", applyFill: "applyFill", applyStroke: "applyStroke", strokeOverFill: "strokeOverFill", baselineShift: "baselineShift",
        autoLeading: "autoLeading", fauxBold: "fauxBold", fauxItalic: "fauxItalic", horizontalScale: "horizontalScale", verticalScale: "verticalScale",
        tsume: "tsume", kerning: "kerning", ligature: "ligature", noBreak: "noBreak", autoHyphenate: "autoHyphenate", everyLineComposer: "everyLineComposer",
        hangingRoman: "hangingRoman", firstLineIndent: "firstLineIndent", startIndent: "startIndent", endIndent: "endIndent",
        spaceBefore: "spaceBefore", spaceAfter: "spaceAfter" };
    var enums = {
        justification: { left: ParagraphJustification.LEFT_JUSTIFY, center: ParagraphJustification.CENTER_JUSTIFY, right: ParagraphJustification.RIGHT_JUSTIFY,
            fullLeft: ParagraphJustification.FULL_JUSTIFY_LASTLINE_LEFT, fullCenter: ParagraphJustification.FULL_JUSTIFY_LASTLINE_CENTER,
            fullRight: ParagraphJustification.FULL_JUSTIFY_LASTLINE_RIGHT, fullLastLineFull: ParagraphJustification.FULL_JUSTIFY_LASTLINE_FULL },
        fontBaselineOption: { normal: FontBaselineOption.FONT_NORMAL_BASELINE, superscript: FontBaselineOption.FONT_FAUXED_SUPERSCRIPT, subscript: FontBaselineOption.FONT_FAUXED_SUBSCRIPT },
        autoKernType: { none: AutoKernType.NO_AUTO_KERN, metric: AutoKernType.METRIC_KERN, optical: AutoKernType.OPTICAL_KERN },
        lineJoinType: { miter: LineJoinType.LINE_JOIN_MITER, round: LineJoinType.LINE_JOIN_ROUND, bevel: LineJoinType.LINE_JOIN_BEVEL },
        direction: { leftToRight: ParagraphDirection.DIRECTION_LEFT_TO_RIGHT, rightToLeft: ParagraphDirection.DIRECTION_RIGHT_TO_LEFT },
        digitSet: { "default": DigitSet.DEFAULT_DIGITS, arabic: DigitSet.ARABIC_DIGITS, hindi: DigitSet.HINDI_DIGITS },
        leadingType: { roman: LeadingType.ROMAN_LEADING_TYPE, japanese: LeadingType.JAPANESE_LEADING_TYPE },
        baselineDirection: { withStream: BaselineDirection.BASELINE_WITH_STREAM, verticalRotated: BaselineDirection.BASELINE_VERTICAL_ROTATED,
            verticalCrossStream: BaselineDirection.BASELINE_VERTICAL_CROSS_STREAM },
        lineOrientation: { horizontal: LineOrientation.HORIZONTAL, verticalRtl: LineOrientation.VERTICAL_RIGHT_TO_LEFT, verticalLtr: LineOrientation.VERTICAL_LEFT_TO_RIGHT }
    };
    for (var k in o) {
        if (k === "allCaps") { td.fontCapsOption = o[k] ? FontCapsOption.FONT_ALL_CAPS : FontCapsOption.FONT_NORMAL_CAPS; }
        else if (k === "smallCaps") { td.fontCapsOption = o[k] ? FontCapsOption.FONT_SMALL_CAPS : FontCapsOption.FONT_NORMAL_CAPS; }
        else if (k === "autoKernType" || k === "justification" || enums[k]) { td[k] = enums[k][o[k]]; }
        else { td[map[k]] = o[k]; }
    }
}

function runStep(comp, s) {
    var op = s[0], t, td, prop;
    if (op === "text") {
        t = s[2].box ? comp.layers.addBoxText(s[2].box, s[1]) : comp.layers.addText(s[1]);
        t.name = "T";
        if (s[2].boxPos) { td = t.property("ADBE Text Properties").property("ADBE Text Document").value; td.boxTextPos = s[2].boxPos; t.property("ADBE Text Properties").property("ADBE Text Document").setValue(td); }
        return;
    }
    t = findText(comp);
    var src = t.property("ADBE Text Properties").property("ADBE Text Document");
    if (op === "content") { td = src.value; td.text = s[1]; src.setValue(td); }
    else if (op === "style") { td = src.value; applyAttrs(td, s[1]); src.setValue(td); }
    else if (op === "range") { td = src.value; applyAttrs(td.characterRange(s[1], s[2]), s[3]); src.setValue(td); }
    else if (op === "reset") { td = src.value; if (s[1] !== "paragraph") { td.resetCharStyle(); } if (s[1] !== "character") { td.resetParagraphStyle(); } src.setValue(td); }
    else if (op === "anim") {
        var an = t.property("ADBE Text Properties").property("ADBE Text Animators").addProperty("ADBE Text Animator");
        an.name = s[1];
        an.property("ADBE Text Selectors").addProperty("ADBE Text Selector");
        for (var m in s[2]) { an.property("ADBE Text Animator Properties").addProperty(m).setValue(s[2][m]); }
    } else if (op === "sel" || op === "skey" || op === "get") {
        prop = t.property("ADBE Text Properties").property("ADBE Text Animators").property(s[1]).property("ADBE Text Selectors").property(1);
        prop = ADVANCED[s[2]] ? prop.property("ADBE Text Range Advanced").property(s[2]) : prop.property(s[2]);
        if (op === "get") { prop.valueAtTime(0, false); }
        else if (op === "sel") { prop.setValue(s[3]); }
        else {
            for (var i = 0; i < s[3].length; i++) { prop.setValueAtTime(s[3][i][0], s[3][i][1]); }
            var e = [new KeyframeEase(0, 33.333333)];
            for (var j = 1; j <= prop.numKeys; j++) { prop.setTemporalEaseAtKey(j, e, e); }
        }
    } else if (op === "rmanim") { t.property("ADBE Text Properties").property("ADBE Text Animators").property(s[1]).remove(); }
    else if (op === "anchor") { t.property("ADBE Transform Group").property("ADBE Anchor Point").setValue(s[1]); }
    else if (op === "mask") {
        var sh = new Shape(), cx = s[1][0], cy = s[1][1], hw = s[2][0] / 2, hh = s[2][1] / 2;
        sh.vertices = [[cx - hw, cy - hh], [cx + hw, cy - hh], [cx + hw, cy + hh], [cx - hw, cy + hh]];
        sh.closed = true;
        t.property("ADBE Mask Parade").addProperty("ADBE Mask Atom").property("ADBE Mask Shape").setValue(sh);
    } else { throw new Error("unknown step " + op); }
}

function main() {
    app.project.close(CloseOptions.DO_NOT_SAVE_CHANGES);
    app.open(new File(EMPTY_AEP));
    var base = app.project.items.addComp(BASE, 1920, 1080, 1, 5, 25);
    base.bgColor = [0, 0, 0];
    base.layers.addSolid([1, 0, 0], "L", 1920, 1080, 1);
    for (var i = 0; i < CASES.length; i++) {
        var comp = base.duplicate();
        comp.name = CASES[i].id;
        for (var j = 0; j < CASES[i].steps.length; j++) { runStep(comp, CASES[i].steps[j]); }
    }
    app.project.save(new File(OUT_AEP)); // then patch-text.py for 732/733
    return CASES.length + 1;
}

if (typeof app !== "undefined" && typeof EC_DATA_ONLY === "undefined") {
    main();
}
