// Size, frame rate and duration of the comps listed in CFG.comps (read-only).
var info = {};
for (var i = 0; i < CFG.comps.length; i++) {
  var c = compByName(CFG.comps[i]);
  if (c) info[c.name] = { width: c.width, height: c.height, fps: c.frameRate, duration: c.duration, layers: c.numLayers };
}
emit(info);
