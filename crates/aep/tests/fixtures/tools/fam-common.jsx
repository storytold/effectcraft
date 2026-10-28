// fam-common.jsx - shared helpers of the combined family generators (3d, footage-precomp,
// markers, settings). ES3 data only: every family file defines PROJECTS, a list of
//   { out: "<file>.aep", settings: {...} | null, setup: [op...], cases: [{ id, steps: [op...] }] }
// where an op is an After Effects MCP atomic operation { operation, args } (ae_do / batch.run).
// In case steps the comp name "$" stands for the case comp; a value "@key" stands for the
// project item id that the setup op tagged with `key` (an import or a helper comp), filled in by
// fam-to-mcp.js from the setup batch's results. Every case comp starts as a duplicate of the
// base comp "000-base" (comp C of synthetic/000-base.aep: 1920x1080, 25 fps, 5 s, black, one red
// 1920x1080 solid "L"), so the importer's result for the old fixture's comp C must equal the case
// comp here.

var MEDIA = "<corpus>/synthetic/";
var BASE = "000-base";

function op(operation, args) { return { operation: operation, args: args }; }
// The template comp, exactly as the pilot (effects.jsx / to-mcp.js) builds it.
function baseTemplate() {
    return [
        op("comp.create", { name: BASE, width: 1920, height: 1080, fps: 25, duration: 5, bgColor: [0, 0, 0] }),
        op("layer.create_solid", { comp: BASE, name: "L", color: [1, 0, 0], width: 1920, height: 1080 })
    ];
}
// A tagged setup op: its result's item id is bound to `key`.
function tag(key, o) { o.tag = key; return o; }
function imp(key, rel, extra) {
    var a = { path: MEDIA + rel };
    if (extra) for (var k in extra) a[k] = extra[k];
    return tag(key, op("project.import_file", a));
}
function setp(layer, path, value) { return op("property.set", { comp: "$", layer: layer, property: path, value: value }); }
function getp(layer, path, t) { return op("property.get", { comp: "$", layer: layer, property: path, time: t }); }
function props(layer, p) { return op("layer.set_props", { comp: "$", layer: layer, props: p }); }
function footage(key, name) {
    var a = { comp: "$", sourceItemId: "@" + key };
    if (name) a.name = name;
    return op("layer.create_footage", a);
}
