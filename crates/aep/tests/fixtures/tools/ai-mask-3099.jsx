// Issue 3099: how After Effects sizes a placed .ai (PDF) and where masks / parented layers land.
// Generates ai-mask-3099.aep: run `python -I ai-mask-3099.py <work>` first, set `dir` to that
// folder, then run this script (ExtendScript) in an empty, untitled project. It returns After
// Effects' read-back (footage sizes, transforms, mask vertices, toComp points; the tests compare
// against it) and saves <work>/i3099-ai-mask.aep, which is then scrubbed (README: steps 5 and 6).
(function () {
    var dir = "<work>/";
    var out = {};
    if (app.project.file !== null || app.project.numItems !== 0) {
        return "ABORT: project not empty/untitled (" + app.project.numItems + " items)";
    }
    var target = new File(dir + "i3099-ai-mask.aep");
    if (target.exists) { return "ABORT: target exists"; }
    var names = ["i3099-a-boxes.ai", "i3099-b-frac.ai", "i3099-c-rot.ai", "i3099-d-trim.ai"];
    var items = {};
    out.footage = {};
    for (var i = 0; i < names.length; i++) {
        var io = new ImportOptions(new File(dir + names[i]));
        io.importAs = ImportAsType.FOOTAGE;
        var it = app.project.importFile(io);
        items[names[i]] = it;
        out.footage[names[i]] = { w: it.width, h: it.height, par: it.pixelAspect };
    }
    var comp = app.project.items.addComp("ai-mask", 1920, 1080, 1, 5, 25);
    var art = comp.layers.add(items["i3099-a-boxes.ai"]);
    art.name = "art";
    var artD = comp.layers.add(items["i3099-d-trim.ai"]);
    artD.name = "artD";
    artD.collapseTransformation = true;
    var artC = comp.layers.add(items["i3099-c-rot.ai"]);
    artC.name = "artC";
    artC.property("ADBE Transform Group").property("ADBE Position").setValue([1500, 800]);
    var n1 = comp.layers.addNull(); n1.name = "N1";
    var n2 = comp.layers.addNull(); n2.name = "N2";
    n2.setParentWithJump(n1);
    art.setParentWithJump(n2);
    artD.setParentWithJump(n2);
    var t = function (l, p, v) { l.property("ADBE Transform Group").property(p).setValue(v); };
    t(n1, "ADBE Position", [700, 400]);
    t(n1, "ADBE Rotate Z", 15);
    t(n1, "ADBE Scale", [80, 120]);
    t(n2, "ADBE Anchor Point", [10, 20]);
    t(n2, "ADBE Position", [50, -30]);
    t(n2, "ADBE Scale", [150, 150]);
    t(art, "ADBE Position", [0, 0]);
    t(artD, "ADBE Position", [300, 250]);
    // Two rectangular masks on each art layer, in layer pixels.
    var rect = function (l, nm, x0, y0, x1, y1) {
        var m = l.property("ADBE Mask Parade").addProperty("ADBE Mask Atom");
        m.name = nm;
        var s = new Shape();
        s.vertices = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
        s.closed = true;
        m.property("ADBE Mask Shape").setValue(s);
    };
    rect(art, "M1", 40, 30, 200, 90);
    rect(art, "M2", 220, 100, 350, 160);
    rect(artD, "M1", 40, 30, 200, 90);
    rect(artD, "M2", 220, 100, 350, 160);
    // World positions of layer points through the parent chain (toComp), read via a Point Control.
    var probe = n1.property("ADBE Effect Parade").addProperty("ADBE Point Control");
    var pt = probe.property(1);
    var toComp = function (ln, p) {
        pt.expression = "thisComp.layer('" + ln + "').toComp([" + p[0] + "," + p[1] + "])";
        var v = pt.valueAtTime(0, false);
        return [v[0], v[1]];
    };
    var rd = function (l) {
        var tg = l.property("ADBE Transform Group");
        var r = {
            w: l.width, h: l.height,
            anchor: tg.property("ADBE Anchor Point").value,
            position: tg.property("ADBE Position").value,
            scale: tg.property("ADBE Scale").value,
            rot: tg.property("ADBE Rotate Z").value,
            masks: []
        };
        var mp = l.property("ADBE Mask Parade");
        for (var k = 1; k <= mp.numProperties; k++) {
            r.masks.push(mp.property(k).property("ADBE Mask Shape").value.vertices);
        }
        return r;
    };
    out.layers = { art: rd(art), artD: rd(artD), artC: rd(artC), N1: rd(n1), N2: rd(n2) };
    out.world = {};
    var pts = { tl: [0, 0], tr: [art.width, 0], bl: [0, art.height], m1: [40, 30], m2: [350, 160] };
    for (var key in pts) { out.world["art_" + key] = toComp("art", pts[key]); }
    var ptsD = { tl: [0, 0], tr: [artD.width, 0], bl: [0, artD.height], m1: [40, 30] };
    for (var key2 in ptsD) { out.world["artD_" + key2] = toComp("artD", ptsD[key2]); }
    probe.remove();
    app.project.save(target);
    out.saved = target.fsName;
    return out;
})();
