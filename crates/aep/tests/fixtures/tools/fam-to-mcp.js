// Turns a family table (3d.jsx, footage-precomp.jsx, markers.jsx, settings.jsx; helpers in
// fam-common.jsx) into After Effects MCP batch.run operation lists (eval.run is disabled on the MCP).
//
// Usage:
//   node fam-to-mcp.js <family> setup            -> ops-<family>/<out>.setup.json per project
//   node fam-to-mcp.js <family> cases <out> [ids] -> ops-<family>/<out>.cases-NN.json
// `ids` is a JSON file { key: itemId } with the ids the tagged setup ops returned (imports,
// helper comps); it is only needed when a case refers to "@key".
//
// Run sequence per project (instance.dialogs empty):
//   1. ae_save_project path <corpus>/scratch-close.aep
//   2. project.open <corpus>/combined/gen/empty-work.aep (copy of 001-empty.aep)
//   3. batch.run <out>.setup.json; write the tagged ids into ids-<out>.json
//   4. node fam-to-mcp.js <family> cases <out> ids-<out>.json; batch.run each cases-NN.json
//      (check failed == 0)
//   5. ae_save_project path <corpus>/combined/raw/<out>; scrub.py -> combined/<out>
var fs = require("fs");
var path = require("path");
var family = process.argv[2];
var mode = process.argv[3];
var src = fs.readFileSync(path.join(__dirname, "fam-common.jsx"), "utf8") + "\n" + fs.readFileSync(path.join(__dirname, family + ".jsx"), "utf8");
eval(src); // our own generator files (trusted, local): define PROJECTS and the helpers
var outDir = path.join(__dirname, "ops-" + family);
fs.mkdirSync(outDir, { recursive: true });

function clone(v) { return JSON.parse(JSON.stringify(v)); }
function subst(v, comp, ids) {
    if (typeof v === "string") {
        if (v === "$") return comp;
        if (v.charAt(0) === "@" && v.length > 1) {
            var k = v.slice(1);
            if (!(k in ids)) throw new Error("no id for " + v);
            return ids[k];
        }
        return v;
    }
    if (Array.isArray(v)) return v.map(function (x) { return subst(x, comp, ids); });
    if (v && typeof v === "object") {
        var o = {};
        for (var key in v) o[key] = subst(v[key], comp, ids);
        return o;
    }
    return v;
}

if (mode === "setup") {
    PROJECTS.forEach(function (p) {
        // Settings first (right after the base template), so footage is imported under them.
        var setup = p.setup.slice(0, 2);
        if (p.settings) setup.push({ operation: "project.set_settings", args: p.settings });
        setup = setup.concat(p.setup.slice(2));
        var ops = setup.map(function (o) { var c = clone(o); delete c.tag; return c; });
        ops.push({ operation: "project.get_settings", args: {} });
        var tags = setup.map(function (o, i) { return o.tag ? i + ":" + o.tag : null; }).filter(Boolean);
        fs.writeFileSync(path.join(outDir, p.out + ".setup.json"), JSON.stringify(ops));
        console.log(p.out, "setup ops", ops.length, "tags (result index:key)", tags.join(" "));
    });
} else if (mode === "cases") {
    var out = process.argv[4];
    var ids = process.argv[5] ? JSON.parse(fs.readFileSync(process.argv[5], "utf8")) : {};
    var p = PROJECTS.filter(function (x) { return x.out === out; })[0];
    if (!p) throw new Error("no project " + out);
    var PER = 12, n = 1, cur = [], count = 0;
    var flush = function () {
        if (!cur.length) return;
        fs.writeFileSync(path.join(outDir, out + ".cases-" + (n < 10 ? "0" : "") + n + ".json"), JSON.stringify(cur));
        n++; cur = []; count = 0;
    };
    p.cases.forEach(function (c) {
        cur.push({ operation: "comp.duplicate", args: { comp: BASE, newName: c.id } });
        c.steps.forEach(function (s) { cur.push(subst(clone(s), c.id, ids)); });
        if (++count >= PER) flush();
    });
    flush();
    console.log(out, "case batches", n - 1, "cases", p.cases.length);
} else {
    throw new Error("usage: node fam-to-mcp.js <family> setup|cases <out> [ids.json]");
}
