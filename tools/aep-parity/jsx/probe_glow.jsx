// Read-only probe of glow_fit.py: every selected layer (comp, timing, source size, 2D transform) and
// every precomp layer that hosts a comp holding selected layers (host comp, timing, transform).
function v(p) { try { return p.value; } catch (e) { return null; } }
function keyed(p) { try { return p.numKeys > 0 || (p.expressionEnabled && p.expression != ""); } catch (e) { return false; } }
function xf(l) {
  var t = l.property("ADBE Transform Group"), names = ["ADBE Anchor Point", "ADBE Position", "ADBE Scale", "ADBE Rotate Z"];
  var o = { anchor: v(t.property(names[0])), position: v(t.property(names[1])), scale: v(t.property(names[2])),
            rotation: v(t.property(names[3])), animated: false };
  for (var i = 0; i < names.length; i++) if (keyed(t.property(names[i]))) o.animated = true;
  try { o.parent = l.parent ? l.parent.name : null; } catch (e) { o.parent = null; }
  try { o.threeD = !!l.threeDLayer; } catch (e) { o.threeD = false; }
  return o;
}
function timing(l) {
  var o = { startTime: l.startTime, inPoint: l.inPoint, outPoint: l.outPoint, enabled: l.enabled };
  try { o.stretch = l.stretch; } catch (e) { o.stretch = 100; }
  try { o.timeRemap = !!l.timeRemapEnabled; } catch (e) { o.timeRemap = false; }
  return o;
}
var out = { glow: {}, hosts: [] }, ls = selectedLayers();
for (var k = 0; k < ls.length; k++) {
  var c = ls[k][0], l = ls[k][1];
  if (!out.glow[c.name]) out.glow[c.name] = { width: c.width, height: c.height, fps: c.frameRate, duration: c.duration, layers: [] };
  var o = timing(l);
  o.index = l.index; o.name = l.name; o.xf = xf(l);
  o.source = l.source ? [l.source.width, l.source.height] : null;
  out.glow[c.name].layers.push(o);
}
var cs = allComps();
for (var i = 0; i < cs.length; i++) {
  for (var j = 1; j <= cs[i].numLayers; j++) {
    var l = cs[i].layer(j), src = null;
    try { src = l.source; } catch (e) {}
    if (!src || !(src instanceof CompItem) || !out.glow[src.name]) continue;
    var h = timing(l);
    h.host = cs[i].name; h.index = l.index; h.source = src.name; h.xf = xf(l);
    out.hosts.push(h);
  }
}
emit(out);
