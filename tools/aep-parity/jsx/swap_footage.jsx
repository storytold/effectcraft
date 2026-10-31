// Replace footage files (the scripting form of File > Replace Footage): every layer that uses the
// footage item follows. CFG.swaps[i].old is matched against the end of the footage path and must hit
// exactly one item; CFG.swaps[i].new is the replacement file.
var done = 0;
for (var s = 0; s < CFG.swaps.length; s++) {
  var old = CFG.swaps[s].old.replace(/\\/g, "/"), hits = [];
  for (var i = 1; i <= app.project.numItems; i++) {
    var it = app.project.item(i);
    if (!(it instanceof FootageItem) || !it.file) continue;
    var p = it.file.fsName.replace(/\\/g, "/");
    if (p.length >= old.length && p.substring(p.length - old.length) === old &&
        (p.length == old.length || p.charAt(p.length - old.length - 1) == "/")) hits.push(it);
  }
  if (hits.length != 1) throw new Error("swap: " + hits.length + " footage items end with " + old + " (need exactly one)");
  hits[0].replace(new File(CFG.swaps[s].new));
  writeLn("swap: " + old + " -> " + CFG.swaps[s].new);
  done++;
}
writeLn("swap: " + done + " footage items replaced");
