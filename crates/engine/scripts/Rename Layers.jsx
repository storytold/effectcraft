// Rename Layers — an Visualab Effects sample script (original work, MIT OR Apache-2.0).
//
// Renames the selected layers of the active composition (all layers when none are selected):
// find & replace, then an optional prefix, suffix and running number. One undo step.

(function renameLayers() {
  var comp = app.project.activeItem;
  if (!(comp instanceof CompItem)) {
    alert("Rename Layers: open a composition first.");
    return;
  }
  var layers = comp.selectedLayers;
  if (layers.length === 0) {
    layers = [];
    for (var i = 1; i <= comp.numLayers; i++) layers.push(comp.layer(i));
  }

  var dlg = new Window("dialog", "Rename Layers");
  dlg.orientation = "column";
  dlg.alignChildren = ["fill", "top"];

  var find = dlg.add("panel", undefined, "Find and Replace");
  find.alignChildren = ["fill", "top"];
  var row1 = find.add("group");
  row1.add("statictext", undefined, "Find:");
  var findText = row1.add("edittext", undefined, "");
  findText.characters = 20;
  var row2 = find.add("group");
  row2.add("statictext", undefined, "Replace:");
  var replaceText = row2.add("edittext", undefined, "");
  replaceText.characters = 20;

  var extra = dlg.add("panel", undefined, "Add");
  extra.alignChildren = ["fill", "top"];
  var row3 = extra.add("group");
  row3.add("statictext", undefined, "Prefix:");
  var prefix = row3.add("edittext", undefined, "");
  prefix.characters = 12;
  row3.add("statictext", undefined, "Suffix:");
  var suffix = row3.add("edittext", undefined, "");
  suffix.characters = 12;
  var number = extra.add("checkbox", undefined, "Number the layers");
  var row4 = extra.add("group");
  row4.add("statictext", undefined, "Start at:");
  var start = row4.add("edittext", undefined, "1");
  start.characters = 5;
  start.enabled = false;
  number.onClick = function () {
    start.enabled = number.value;
  };

  var buttons = dlg.add("group");
  buttons.alignment = "right";
  buttons.add("button", undefined, "Cancel", { name: "cancel" });
  buttons.add("button", undefined, "OK", { name: "ok" });

  if (dlg.show() !== 1) return;

  app.beginUndoGroup("Rename Layers");
  var n = parseInt(start.text, 10);
  if (isNaN(n)) n = 1;
  for (var k = 0; k < layers.length; k++) {
    var name = layers[k].name;
    if (findText.text !== "") name = name.split(findText.text).join(replaceText.text);
    name = prefix.text + name + suffix.text;
    if (number.value) name += " " + (n + k);
    layers[k].name = name;
  }
  app.endUndoGroup();
  writeLn("Renamed " + layers.length + " layer(s).");
})();
