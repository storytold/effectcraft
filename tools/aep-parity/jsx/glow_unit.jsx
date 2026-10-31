// Transfer probe of glow_fit.py: the selected layers' sources become a flat CFG.level grey field (their
// effects off; masks, opacity, blending mode, parenting and timing kept), so a render minus the "no
// glow" render measures, per pixel, what the rest of the chain does to that layer's content.
// CFG.only (a comp name): only that comp's selected layers get the field; the others are disabled.
var G = CFG.level, ls = selectedLayers(), units = {}, n = 0, off = 0, need = 1;
for (var k = 0; k < ls.length; k++) need = Math.max(need, ls[k][0].duration - ls[k][1].startTime + 1);
function unitComp(w, h, fps) {
  var key = w + "x" + h;
  if (!units[key]) {
    var u = app.project.items.addComp("GLOWUNIT " + key, w, h, 1, need, fps);
    u.layers.addSolid([G, G, G], "unit", w, h, 1);
    units[key] = u;
  }
  return units[key];
}
for (var k = 0; k < ls.length; k++) {
  var c = ls[k][0], l = ls[k][1];
  if (CFG.only && c.name != CFG.only) { l.enabled = false; off++; continue; }
  if (!l.source) { writeLn("glow unit: " + c.name + " / " + l.name + " has no source, skipped"); continue; }
  effectsOff(l);
  l.replaceSource(unitComp(l.source.width, l.source.height, c.frameRate), false);
  n++;
}
writeLn("glow unit: " + n + " flat, " + off + " off");
