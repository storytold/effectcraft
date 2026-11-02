// Render Queue Batch — an Visualab Effects sample script (original work, MIT OR Apache-2.0).
//
// Adds the compositions selected in the Project panel (every composition when none are
// selected) to the Render Queue, each with an output file named after the comp in a folder you
// choose, and optionally starts rendering.

(function renderQueueBatch() {
  var comps = [];
  var sel = app.project.selection;
  for (var i = 0; i < sel.length; i++) if (sel[i] instanceof CompItem) comps.push(sel[i]);
  if (comps.length === 0) {
    for (var j = 1; j <= app.project.numItems; j++) {
      var it = app.project.item(j);
      if (it instanceof CompItem) comps.push(it);
    }
  }
  if (comps.length === 0) {
    alert("Render Queue Batch: the project has no compositions.");
    return;
  }

  var dlg = new Window("dialog", "Render Queue Batch");
  dlg.alignChildren = ["fill", "top"];
  dlg.add("statictext", undefined, comps.length + " composition(s) will be queued.");
  var row = dlg.add("group");
  row.add("statictext", undefined, "Output folder:");
  var folder = row.add("edittext", undefined, Folder.desktop ? Folder.desktop.fsName : "");
  folder.characters = 30;
  var fmtRow = dlg.add("group");
  fmtRow.add("statictext", undefined, "Format:");
  var format = fmtRow.add("dropdownlist", undefined, ["H.264 (.mp4)", "PNG Sequence"]);
  format.selection = 0;
  var go = dlg.add("checkbox", undefined, "Start rendering");
  var buttons = dlg.add("group");
  buttons.alignment = "right";
  buttons.add("button", undefined, "Cancel", { name: "cancel" });
  buttons.add("button", undefined, "Queue", { name: "ok" });
  if (dlg.show() !== 1) return;

  var png = format.selection && format.selection.index === 1;
  app.beginUndoGroup("Render Queue Batch");
  for (var k = 0; k < comps.length; k++) {
    var rq = app.project.renderQueue.items.add(comps[k]);
    var name = comps[k].name.replace(/[\\\/:*?"<>|]/g, "_");
    var om = rq.outputModule(1);
    if (png) om.format = "png";
    om.file = new File(folder.text + "/" + name + (png ? "_[#####].png" : ".mp4"));
  }
  app.endUndoGroup();
  writeLn("Queued " + comps.length + " composition(s).");
  if (go.value) app.project.renderQueue.render();
})();
