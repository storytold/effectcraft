// Fitted glow fields, written by glow_fit.py (`emit`). For each comp in CFG.fields it builds
// CFG.prefix + <comp>, sized like the selected layers' source: an opaque black base, then one black
// 2R x 2R solid per cone centred on the cone, with a radial Gradient Ramp (Start = centre, End = centre
// + R, End Color black, Start Color keyed to the fitted RGB amplitude), stacked in Add mode. The
// selected layers of that comp take it as their source with their effects off; masks, opacity keys,
// blending mode, parenting and timing stay, so the field goes through the same chain as the original
// content. Key times are field-comp times (the comp's time minus the layer's start time).
var made = 0, swapped = 0, cones = 0;
for (var g in CFG.fields) {
  var d = CFG.fields[g], host = compByName(g);
  if (!host) { writeLn("glow overrides: no comp " + g); continue; }
  var dur = 1;
  for (var q = 0; q < d.t.length; q++) dur = Math.max(dur, d.t[q] + 1);
  dur = Math.max(dur, host.duration - d.start + 1);
  var f = app.project.items.addComp(CFG.prefix + g, d.w, d.h, 1, dur, host.frameRate);
  made++;
  f.layers.addSolid([0, 0, 0], "base", d.w, d.h, 1);
  var side = Math.ceil(2 * d.r);
  for (var b = 0; b < d.b.length; b++) {
    var bl = d.b[b];
    var s = f.layers.addSolid([0, 0, 0], "cone " + b, side, side, 1);
    s.blendingMode = BlendingMode.ADD;
    s.property("ADBE Transform Group").property("ADBE Position").setValue([bl.c[0], bl.c[1]]);
    var fx = s.property("ADBE Effect Parade").addProperty("ADBE Ramp");
    fx.property("Ramp Shape").setValue(2);
    fx.property("Start of Ramp").setValue([side / 2, side / 2]);
    fx.property("End of Ramp").setValue([side / 2 + d.r, side / 2]);
    fx.property("End Color").setValue([0, 0, 0, 1]);
    var vs = [];
    for (var k = 0; k < bl.v.length; k++) vs.push([bl.v[k][0], bl.v[k][1], bl.v[k][2], 1]);
    if (d.t.length == 1) fx.property("Start Color").setValue(vs[0]);
    else fx.property("Start Color").setValuesAtTimes(d.t, vs);
    cones++;
  }
  for (var j = 1; j <= host.numLayers; j++) {
    var l = host.layer(j);
    if (!isSelected(host, l)) continue;
    effectsOff(l);
    l.replaceSource(f, false);
    swapped++;
  }
}
writeLn("glow overrides: " + made + " fields, " + cones + " cones, " + swapped + " layers swapped");
