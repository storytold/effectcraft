// Read-only inventory to write a parity config: every comp (size, rate, duration, layer count), the
// effects used in the project by matchName, the fonts of text layers, and the layers of the comps
// listed in CFG.comps (name, switches, source, timing, effects).
var cs = allComps(), fx = {}, fonts = {};
writeLn("== comps");
for (var i = 0; i < cs.length; i++) {
  var c = cs[i];
  writeLn(c.name + " | " + c.width + "x" + c.height + " | " + c.frameRate + " fps | " + c.duration.toFixed(3) + " s | " + c.numLayers + " layers");
  for (var j = 1; j <= c.numLayers; j++) {
    var l = c.layer(j), p = effectsOf(l);
    if (p) for (var e = 1; e <= p.numProperties; e++) { var k = p.property(e).matchName + " (" + p.property(e).name + ")"; fx[k] = (fx[k] || 0) + 1; }
    if (l instanceof TextLayer) {
      try { var d = l.property("ADBE Text Properties").property("ADBE Text Document").value; fonts[d.font] = (fonts[d.font] || 0) + 1; } catch (er) {}
    }
  }
}
writeLn("== effects (matchName (name): instances)");
for (var k in fx) writeLn(fx[k] + "  " + k);
writeLn("== fonts (text layers)");
for (var k in fonts) writeLn(fonts[k] + "  " + k);
var want = CFG.comps || [];
for (var q = 0; q < want.length; q++) {
  var c = compByName(want[q]);
  if (!c) { writeLn("== no comp " + want[q]); continue; }
  writeLn("== layers of " + c.name);
  for (var j = 1; j <= c.numLayers; j++) {
    var l = c.layer(j), s = "#" + j + " " + l.name + " | en " + l.enabled;
    try { s += " 3D " + l.threeDLayer; } catch (er) {}
    try { s += " | src " + (l.source ? l.source.name : "-"); } catch (er) {}
    s += " | in " + l.inPoint.toFixed(3) + " out " + l.outPoint.toFixed(3) + " st " + l.startTime.toFixed(3);
    var p = effectsOf(l);
    if (p && p.numProperties) { var n = []; for (var e = 1; e <= p.numProperties; e++) n.push(p.property(e).matchName); s += " | fx " + n.join(", "); }
    writeLn(s);
  }
}
