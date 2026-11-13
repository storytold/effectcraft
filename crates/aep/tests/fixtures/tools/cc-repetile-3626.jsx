// Issue 3626: how After Effects stores CC RepeTile. Generates cc-repetile-3626.aep: set `dir` to a
// work folder, then run this script (ExtendScript) in After Effects with no project open, or an
// empty untitled one. It saves <work>/cc-repetile-3626.aep (scrubbed afterwards: README steps 5
// and 6) and writes After Effects' read-back of every parameter to <work>/cc-repetile-3626.txt.
// Each composition is the corpus base (1920x1080, 25 fps, 5 s, black, one red 1920x1080 solid L)
// with CC RepeTile on L.
(function () {
    var dir = "<work>/";
    var log = new File(dir + "cc-repetile-3626.txt");
    var lines = [];
    var done = function (msg) {
        lines.push(msg);
        log.encoding = "UTF-8";
        log.open("w");
        log.write(lines.join("\n") + "\n");
        log.close();
    };
    if (app.project.file !== null || app.project.numItems !== 0) {
        return done("ABORT: project not empty/untitled (" + app.project.numItems + " items)");
    }
    var target = new File(dir + "cc-repetile-3626.aep");
    if (target.exists) { return done("ABORT: target exists"); }
    var make = function (name) {
        var comp = app.project.items.addComp(name, 1920, 1080, 1, 5, 25);
        comp.bgColor = [0, 0, 0];
        var l = comp.layers.addSolid([1, 0, 0], "L", 1920, 1080, 1);
        var fx = l.property("ADBE Effect Parade").addProperty("CC RepeTile");
        return fx;
    };
    var p = function (fx, i) { return fx.property("CC RepeTile-000" + i); };
    // Expand Right/Left/Down/Up, Tiling Unfold (item 4), Blend Borders 25 % (scripting takes 0..1).
    var a = make("repetile-values");
    p(a, 1).setValue(120);
    p(a, 2).setValue(80);
    p(a, 3).setValue(40);
    p(a, 4).setValue(20);
    p(a, 5).setValue(4);
    p(a, 6).setValue(0.25);
    // Every parameter at its default.
    make("repetile-defaults");
    // Tiling Rosette (item 9, no counterpart in EffectCraft); Expand Right keyed 0 -> 300 over 1 s..2 s.
    var c = make("repetile-rosette");
    p(c, 5).setValue(9);
    p(c, 1).setValueAtTime(1, 0);
    p(c, 1).setValueAtTime(2, 300);
    // Expand Left 9838, above EffectCraft's slider range.
    var d = make("repetile-wide");
    p(d, 2).setValue(9838);
    for (var i = 1; i <= app.project.numItems; i++) {
        var it = app.project.item(i);
        if (!(it instanceof CompItem)) { continue; }
        var fx = it.layer(1).property("ADBE Effect Parade").property(1);
        var vals = [];
        for (var k = 1; k <= 6; k++) {
            var q = p(fx, k);
            var v = q.numKeys > 0 ? "keys " + q.keyValue(1) + "@" + q.keyTime(1) + " " + q.keyValue(2) + "@" + q.keyTime(2) : String(q.value);
            vals.push(q.name + "=" + v);
        }
        lines.push(it.name + ": " + vals.join(", "));
    }
    app.project.save(target);
    done("SAVED " + target.fsName);
})();
