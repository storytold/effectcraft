// Original behavior probe: the spatial tangents a host gives auto-Bezier Position keys.
// Runs unchanged in After Effects (File > Scripts > Run Script File, or AfterFX.exe -r) and in
// EffectCraft (effectcraft-cli script examples/ae-spatial-tangents.jsx --empty). Builds three
// original comps with one solid each; run it in a scratch project. EffectCraft prints one JSON line
// per key; After Effects writes them to ae-spatial-tangents.jsonl next to this script.
// No Adobe assets, presets, application internals or implementation code are read.
var EC = String(app.version).indexOf("EffectCraft") >= 0;
var OUT = null;
if (!EC) {
    OUT = new File(File($.fileName).parent.fsName + "/ae-spatial-tangents.jsonl");
    OUT.encoding = "UTF-8";
    OUT.open("w");
}
function emit(s) { if (EC) { writeLn(s); } else { OUT.writeln(s); } }
function r(v) { var o = []; for (var i = 0; i < v.length; i++) o.push(Math.round(v[i] * 1000) / 1000); return "[" + o.join(",") + "]"; }
function dump(name, pts, times) {
    var c = app.project.items.addComp(name, 1920, 1080, 1, 2, 30);
    var p = c.layers.addSolid([1, 1, 1], "S", 100, 100, 1).property("Transform").property("Position");
    for (var i = 0; i < pts.length; i++) p.setValueAtTime(times[i], pts[i]);
    // Pin what a preference could decide: AE can default new keys to linear spatial interpolation.
    for (var k = 1; k <= p.numKeys; k++) p.setSpatialAutoBezierAtKey(k, true);
    for (var k2 = 1; k2 <= p.numKeys; k2++) emit('{"case":"' + name + '","key":' + k2 + ',"in":' + r(p.keyInSpatialTangent(k2)) + ',"out":' + r(p.keyOutSpatialTangent(k2)) + "}");
}
dump("T1 symmetric", [[200, 200], [960, 900], [1700, 200]], [0, 1, 2]);
dump("T2 uneven", [[100, 540], [400, 700], [1800, 540]], [0, 0.5, 2]);
dump("T3 four keys", [[100, 100], [500, 900], [900, 100], [1800, 600]], [0, 0.6, 1.2, 2]);
if (OUT) OUT.close();
