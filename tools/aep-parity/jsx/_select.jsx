// Shared helpers, prepended (after `var CFG = {...};`) to every generated script of the parity tools.
// CFG.select = {layer_name?, effect_match_name?, comps?}: a layer is selected when every given field
// matches (its name, one of its effects' matchName, the name of the comp that holds it).
function compByName(n) {
  for (var i = 1; i <= app.project.numItems; i++) {
    var it = app.project.item(i);
    if (it instanceof CompItem && it.name == n) return it;
  }
  return null;
}
function allComps() {
  var out = [];
  for (var i = 1; i <= app.project.numItems; i++) {
    var it = app.project.item(i);
    if (it instanceof CompItem) out.push(it);
  }
  return out;
}
function effectsOf(l) {
  try { return l.property("ADBE Effect Parade"); } catch (e) { return null; }
}
function hasEffect(l, mn) {
  var p = effectsOf(l);
  if (!p) return false;
  for (var e = 1; e <= p.numProperties; e++) if (p.property(e).matchName == mn) return true;
  return false;
}
function isSelected(comp, l) {
  var s = CFG.select || {};
  if (!s.layer_name && !s.effect_match_name) return false;
  if (s.comps && s.comps.length && s.comps.indexOf(comp.name) < 0) return false;
  if (s.layer_name && l.name != s.layer_name) return false;
  if (s.effect_match_name && !hasEffect(l, s.effect_match_name)) return false;
  return true;
}
function selectedLayers() {
  var out = [], cs = allComps();
  for (var i = 0; i < cs.length; i++)
    for (var j = 1; j <= cs[i].numLayers; j++) if (isSelected(cs[i], cs[i].layer(j))) out.push([cs[i], cs[i].layer(j)]);
  return out;
}
function effectsOff(l) {
  var p = effectsOf(l);
  if (!p) return;
  for (var e = p.numProperties; e >= 1; e--) p.property(e).enabled = false;
}
function emit(obj) { writeLn("@@JSON " + JSON.stringify(obj)); }
