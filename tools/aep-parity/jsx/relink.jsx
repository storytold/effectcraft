// Repoint footage whose path starts with CFG.pairs[i].from to CFG.pairs[i].to + the rest of the path
// (a collected project keeps the author's absolute paths). File.exists is false for every path in the
// CLI script host, so nothing is checked here: a wrong prefix shows up as black footage in renders.
var ok = 0, other = 0;
for (var i = 1; i <= app.project.numItems; i++) {
  var it = app.project.item(i);
  if (!(it instanceof FootageItem) || !it.file) continue;
  var p = it.file.fsName.replace(/\\/g, "/"), hit = false;
  for (var k = 0; k < CFG.pairs.length; k++) {
    var from = CFG.pairs[k].from.replace(/\\/g, "/");
    if (p.indexOf(from) === 0) { it.replace(new File(CFG.pairs[k].to + p.substring(from.length))); ok++; hit = true; break; }
  }
  if (!hit) other++;
}
writeLn("relink: " + ok + " footage items repointed, " + other + " left as they were");
