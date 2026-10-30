// Turns the case table of <family>.jsx (masks, shapes, styles) into After Effects MCP batch.run
// operation lists. Usage: node mss-to-mcp.js <family> [outdir]
//
// Run sequence (instance.dialogs empty):
//   1. ae_save_project path <corpus>/scratch-close.aep (never project.open on a
//      dirty project), then project.open <corpus>/combined/gen/empty-work.aep
//      (copy of 001-empty.aep); instance.dialogs must be empty
//   2. batch.run ops/<family>/batch-00.json (template comp "000-base")
//   3. for every entry of ops/<family>/merge.json: project.merge {path, folderName}; rename the
//      returned comp "C" (item.set_props by id) to `rename`; instance.dialogs must be empty
//   4. batch.run ops/<family>/batch-01..NN.json (check `failed` == 0 after each)
//   5. batch.run ops/<family>/cleanup.json is not generated: delete the "src-*" comps by id
//      (project.delete_item), then ae_save_project path <OUT_AEP>
//   6. scrub.py <OUT_AEP> <family>-scrubbed.aep synthetic/000-base.aep
var fs = require("fs");
var path = require("path");
var fam = process.argv[2];
var src = fs.readFileSync(path.join(__dirname, fam + ".jsx"), "utf8");
var SOURCES = [], MERGED = [];
eval(src); // our own case table (trusted, local)

var out = process.argv[3] || path.join(__dirname, "ops", fam);
fs.mkdirSync(out, { recursive: true });
var BASE = "000-base";

function subst(v, comp) {
    if (v === "$") return comp;
    if (Array.isArray(v)) return v.map(function (x) { return subst(x, comp); });
    if (v && typeof v === "object") {
        var o = {};
        for (var k in v) o[k] = subst(v[k], comp);
        return o;
    }
    return v;
}

var templates = [
    { operation: "comp.create", args: { name: BASE, width: 1920, height: 1080, fps: 25, duration: 5, bgColor: [0, 0, 0] } },
    { operation: "layer.create_solid", args: { comp: BASE, name: "L", color: [1, 0, 0], width: 1920, height: 1080 } }
];
fs.writeFileSync(path.join(out, "batch-00.json"), JSON.stringify(templates));

var merge = [];
SOURCES.forEach(function (s) { merge.push({ path: s.path, folderName: "src-" + s.key, rename: "src-" + s.key }); });
MERGED.forEach(function (m) { merge.push({ path: m.path, folderName: "orig-" + m.id.slice(0, 3), rename: m.id }); });
fs.writeFileSync(path.join(out, "merge.json"), JSON.stringify(merge, null, 1));

var PER = 8, n = 1, cur = [], count = 0, names = [];
function flush() {
    if (!cur.length) return;
    fs.writeFileSync(path.join(out, "batch-" + (n < 10 ? "0" : "") + n + ".json"), JSON.stringify(cur));
    n++; cur = []; count = 0;
}
CASES.forEach(function (c) {
    var from = !c.from || c.from === BASE ? BASE : "src-" + c.from;
    cur.push({ operation: "comp.duplicate", args: { comp: from, newName: c.id } });
    c.steps.forEach(function (s) { cur.push(subst(s, c.id)); });
    names.push(c.id);
    if (++count >= PER) flush();
});
flush();
fs.writeFileSync(path.join(out, "cases.txt"), EXTRA_CASES.concat(names).concat(MERGED.map(function (m) { return m.id; })).join("\n") + "\n");
console.log(fam, "batches", n - 1, "cases", names.length, "merged", MERGED.length, "sources", SOURCES.length);
