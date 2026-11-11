// Turns the CASES table of effects.jsx into After Effects MCP batch.run operation lists (for
// running the generator through the MCP, where eval.run is disabled).
// Usage: node to-mcp.js <outdir> [stepsItemId]
//
// Run sequence (instance.dialogs empty):
//   1. ae_save_project path <corpus>/scratch-close.aep (never project.open on a
//      dirty untitled project: AE then raises a Save As dialog)
//   2. project.open <corpus>/combined/gen/empty-work.aep (copy of 001-empty.aep);
//      instance.dialogs must be empty
//   3. batch.run ops/batch-00.json (templates 000-base, 520-solid-M-grey-960x540), then
//      ops/smoke.json; ae_save_project path .../combined/effects-smoke.aep; run the Rust check
//      with COMBINED_AEP=that file and COMBINED_CASES=216,390,221,534,323,321,077,220,366
//   4. fix the table, reopen empty-work.aep (step 1-2), batch-00, batch-01..NN one by one
//      (check `failed` == 0 after each; timeoutMs 180000)
//   5. project.import_file test-media/steps.png -> item id N; node to-mcp.js ops N;
//      batch.run ops/footage-905-....json
//   6. ae_save_project path <corpus>/combined/effects.aep
// Writes <outdir>/batch-00.json (templates), batch-NN.json (cases, a few per batch) and, when a
// stepsItemId is given, the footage case(s) that need the imported steps.png item.
var fs = require("fs");
var path = require("path");
var src = fs.readFileSync(path.join(__dirname, "effects.jsx"), "utf8");
var EC_DATA_ONLY = true;
eval(src); // our own generator file (trusted, local): defines CASES, BASE, BASE_M

var out = process.argv[2] || path.join(__dirname, "ops");
var stepsId = process.argv[3] ? Number(process.argv[3]) : null;
fs.mkdirSync(out, { recursive: true });

function ops(comp, st) {
    var op = st[0];
    if (op === "fx") {
        var a = { comp: comp, layer: st[1], matchName: st[2] };
        if (st.length > 3) a.name = st[3];
        return { operation: "effect.add", args: a };
    }
    if (op === "set") return { operation: "effect.set_property", args: { comp: comp, layer: st[1], effectIndex: st[2], property: [st[3]], value: st[4] } };
    if (op === "key") return { operation: "effect.set_property", args: { comp: comp, layer: st[1], effectIndex: st[2], property: [st[3]], value: st[5], time: st[4] } };
    if (op === "expr") return { operation: "expression.set", args: { comp: comp, layer: st[1], property: ["ADBE Effect Parade", st[2], st[3]], expression: st[4] } };
    if (op === "eval") return { operation: "property.get", args: { comp: comp, layer: st[1], property: ["ADBE Effect Parade", st[2], st[3]], time: st[4] } };
    if (op === "move") return { operation: "effect.move", args: { comp: comp, layer: st[1], effectIndex: st[2], toIndex: st[3] } };
    if (op === "items") return { operation: "effect.set_dropdown_items", args: { comp: comp, layer: st[1], effect: st[2], items: st[3] } };
    if (op === "solid") return { operation: "layer.create_solid", args: { comp: comp, name: st[1], color: st[2], width: st[3], height: st[4] } };
    if (op === "null") return { operation: "layer.create_null", args: { comp: comp, name: st[1] } };
    if (op === "footage") return { operation: "layer.create_footage", args: { comp: comp, sourceItemId: stepsId, name: st[1] } };
    if (op === "delete") return { operation: "layer.delete", args: { comp: comp, layer: st[1] } };
    if (op === "video") return { operation: "layer.set_enabled", args: { comp: comp, layer: st[1], enabled: st[2] } };
    throw new Error("unknown step " + op);
}

var templates = [
    { operation: "comp.create", args: { name: BASE, width: 1920, height: 1080, fps: 25, duration: 5, bgColor: [0, 0, 0] } },
    { operation: "layer.create_solid", args: { comp: BASE, name: "L", color: [1, 0, 0], width: 1920, height: 1080 } },
    { operation: "comp.duplicate", args: { comp: BASE, newName: BASE_M } },
    { operation: "layer.create_solid", args: { comp: BASE_M, name: "M", color: [0.5, 0.5, 0.5], width: 960, height: 540 } }
];
fs.writeFileSync(path.join(out, "batch-00.json"), JSON.stringify(templates));

var PER = 16;
var n = 1, cur = [], count = 0, names = [];
function flush() {
    if (cur.length) {
        fs.writeFileSync(path.join(out, "batch-" + (n < 10 ? "0" : "") + n + ".json"), JSON.stringify(cur));
        n++;
        cur = [];
        count = 0;
    }
}
for (var i = 0; i < CASES.length; i++) {
    var c = CASES[i];
    if (c.footage) continue;
    cur.push({ operation: "comp.duplicate", args: { comp: c.base === "520" ? BASE_M : BASE, newName: c.id } });
    for (var j = 0; j < c.steps.length; j++) cur.push(ops(c.id, c.steps[j]));
    names.push(c.id);
    if (++count >= PER) flush();
}
flush();
if (stepsId !== null) {
    for (var k = 0; k < CASES.length; k++) {
        var f = CASES[k];
        if (!f.footage) continue;
        var list = [{ operation: "comp.duplicate", args: { comp: BASE, newName: f.id } }];
        for (var m = 0; m < f.steps.length; m++) list.push(ops(f.id, f.steps[m]));
        fs.writeFileSync(path.join(out, "footage-" + f.id + ".json"), JSON.stringify(list));
    }
}
// Smoke test: one case per step kind, built on top of batch-00 (run on a scratch save first).
var SMOKE = ["216-", "390-", "221-", "534-", "323-", "321-", "077-", "220-", "366-"];
var smoke = [];
for (var q = 0; q < CASES.length; q++) {
    var sc = CASES[q];
    var hit = false;
    for (var r = 0; r < SMOKE.length; r++) if (sc.id.indexOf(SMOKE[r]) === 0) hit = true;
    if (!hit) continue;
    smoke.push({ operation: "comp.duplicate", args: { comp: sc.base === "520" ? BASE_M : BASE, newName: sc.id } });
    for (var u = 0; u < sc.steps.length; u++) smoke.push(ops(sc.id, sc.steps[u]));
}
fs.writeFileSync(path.join(out, "smoke.json"), JSON.stringify(smoke));
console.log("batches", n - 1, "cases", names.length);
