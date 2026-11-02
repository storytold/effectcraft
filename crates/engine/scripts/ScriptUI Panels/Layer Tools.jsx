// Layer Tools — an Visualab Effects sample ScriptUI panel (original work, MIT OR Apache-2.0).
//
// Installed in the ScriptUI Panels folder it docks like any panel (Window ▸ Layer Tools.jsx);
// run from File ▸ Scripts it opens as a floating palette. Buttons act on the selected layers of
// the active composition.

(function layerTools(thisObj) {
  var ui = thisObj instanceof Panel ? thisObj : new Window("palette", "Layer Tools", undefined, { resizeable: true });
  ui.orientation = "column";
  ui.alignChildren = ["fill", "top"];
  ui.spacing = 6;

  function comp() {
    var c = app.project.activeItem;
    if (!(c instanceof CompItem)) {
      status.text = "Open a composition first.";
      return null;
    }
    return c;
  }
  function selected(c) {
    var l = c.selectedLayers;
    if (l.length === 0) status.text = "Select some layers.";
    return l;
  }

  var labelRow = ui.add("group");
  labelRow.add("statictext", undefined, "Opacity:");
  var opacity = labelRow.add("slider", undefined, 100, 0, 100);
  opacity.preferredSize.width = 120;
  var opacityValue = labelRow.add("statictext", undefined, "100%");
  opacityValue.characters = 5;
  opacity.onChanging = function () {
    opacityValue.text = Math.round(opacity.value) + "%";
  };
  opacity.onChange = function () {
    var c = comp();
    if (!c) return;
    var layers = selected(c);
    app.beginUndoGroup("Set Opacity");
    for (var i = 0; i < layers.length; i++) layers[i].transform.opacity.setValue(Math.round(opacity.value));
    app.endUndoGroup();
    status.text = "Opacity " + Math.round(opacity.value) + "% on " + layers.length + " layer(s).";
  };

  var shy = ui.add("checkbox", undefined, "Shy");
  shy.onClick = function () {
    var c = comp();
    if (!c) return;
    var layers = selected(c);
    app.beginUndoGroup("Shy Layers");
    for (var i = 0; i < layers.length; i++) layers[i].shy = shy.value;
    app.endUndoGroup();
  };

  var nameRow = ui.add("group");
  nameRow.alignChildren = ["fill", "center"];
  var newName = nameRow.add("edittext", undefined, "Layer");
  newName.characters = 14;
  var renameBtn = nameRow.add("button", undefined, "Rename");
  renameBtn.onClick = function () {
    var c = comp();
    if (!c) return;
    var layers = selected(c);
    app.beginUndoGroup("Rename Layers");
    for (var i = 0; i < layers.length; i++) layers[i].name = layers.length > 1 ? newName.text + " " + (i + 1) : newName.text;
    app.endUndoGroup();
    status.text = "Renamed " + layers.length + " layer(s).";
  };

  var nullBtn = ui.add("button", undefined, "Null at Centroid");
  nullBtn.onClick = function () {
    var c = comp();
    if (!c) return;
    var layers = selected(c);
    if (layers.length === 0) return;
    var x = 0, y = 0;
    for (var i = 0; i < layers.length; i++) {
      var p = layers[i].transform.position.value;
      x += p[0];
      y += p[1];
    }
    app.beginUndoGroup("Null at Centroid");
    var n = c.layers.addNull();
    n.transform.position.setValue([x / layers.length, y / layers.length]);
    app.endUndoGroup();
    status.text = "Added " + n.name + ".";
  };

  var status = ui.add("statictext", undefined, "Select layers, then pick a tool.");
  status.characters = 30;

  if (ui instanceof Window) {
    ui.center();
    ui.show();
  } else {
    ui.layout.layout(true);
  }
})(this);
