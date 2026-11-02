// Ease Presets: easing curves kept by name and applied to the sides of selected keyframes.
//
// A ScriptUI panel that ships with EffectCraft as an optional extension (Window ▸ Ease
// Presets.jsx). It uses only After Effects' documented scripting API: comp.selectedProperties,
// Property.selectedKeys, keyInTemporalEase / keyOutTemporalEase, setTemporalEaseAtKey with
// KeyframeEase, setInterpolationTypeAtKey, app.settings and ScriptUI. Original work by the
// EffectCraft contributors, MIT OR Apache-2.0.
//
// A curve is the out side of a segment's first key and the in side of its second, in the
// Keyframe Velocity dialog's terms: influence in percent of the segment, and speed relative to the
// segment's average speed (1 = as fast as a straight line, 0 = at rest). Applied to a segment, the
// speeds are scaled to it: per dimension, along the motion path for spatial properties, or as
// overall progress for shapes.
//
// Applied to selected keyframes, In eases each key's incoming side with the curve's in side, Out
// its outgoing side with the curve's out side, In-Out both: a side is eased when it has a
// neighbouring key (selected or not; the neighbour isn't changed), its speeds scaled to the
// segment on that side. So a selected pair gets the whole curve, and a single key works too.
// Roving keys are skipped: they pass speed straight through (their sides stay linear), so an ease
// set on them wouldn't play.
//
// User presets are kept with app.settings (section "Ease Presets", key "userPresets") as JSON:
// {"version": 1, "presets": [{"name": …, "curve": {"outInfluence", "outSpeed", "inInfluence",
// "inSpeed"}}]}.

(function easePresets(thisObj) {
  var SECTION = "Ease Presets";
  var KEY = "userPresets";
  var MAX_NAME = 64;
  var MIN_INFLUENCE = 0.1;
  var MAX_SPEED = 100;
  var BEZIER = KeyframeInterpolationType.BEZIER;
  var HOLD = KeyframeInterpolationType.HOLD;

  function curve(outInfluence, outSpeed, inInfluence, inSpeed) {
    return { outInfluence: outInfluence, outSpeed: outSpeed, inInfluence: inInfluence, inSpeed: inSpeed };
  }

  // Our own presets. "Accelerate" starts slowly, "Decelerate" settles slowly.
  var BUILT_IN = [
    { name: "Linear", curve: curve(100 / 3, 1, 100 / 3, 1) },
    { name: "Smooth", curve: curve(40, 0.25, 40, 0.25) },
    { name: "Ease In-Out Soft", curve: curve(25, 0, 25, 0) },
    { name: "Ease In-Out", curve: curve(50, 0, 50, 0) },
    { name: "Ease In-Out Strong", curve: curve(75, 0, 75, 0) },
    { name: "Accelerate", curve: curve(50, 0, 20, 1.5) },
    { name: "Accelerate Strong", curve: curve(80, 0, 15, 2.5) },
    { name: "Decelerate", curve: curve(20, 1.5, 50, 0) },
    { name: "Decelerate Strong", curve: curve(15, 2.5, 80, 0) },
    { name: "Expo In-Out", curve: curve(88, 0, 88, 0) },
    { name: "Expo Accelerate", curve: curve(75, 0, 12, 7) },
    { name: "Expo Decelerate", curve: curve(12, 7, 75, 0) }
  ];

  // ------------------------------------------------------------------ presets

  function finite(v) {
    return typeof v === "number" && isFinite(v);
  }

  // The curve with influences in 0.1–100 % and speeds within ±100; null when a number is missing.
  function sanitized(c) {
    if (!c || !finite(c.outInfluence) || !finite(c.outSpeed) || !finite(c.inInfluence) || !finite(c.inSpeed)) return null;
    var inf = function (v) { return Math.min(100, Math.max(MIN_INFLUENCE, v)); };
    var speed = function (v) { return Math.min(MAX_SPEED, Math.max(-MAX_SPEED, v)); };
    return curve(inf(c.outInfluence), speed(c.outSpeed), inf(c.inInfluence), speed(c.inSpeed));
  }

  function cleanName(name) {
    var n = String(name === undefined || name === null ? "" : name).replace(/^\s+|\s+$/g, "");
    return n.length > 0 && n.length <= MAX_NAME ? n : null;
  }

  function same(a, b) {
    return a.toLowerCase() === b.toLowerCase();
  }

  function find(list, name) {
    for (var i = 0; i < list.length; i++) if (same(list[i].name, name)) return i;
    return -1;
  }

  // User presets; entries that don't parse, have no usable name or repeat a name are skipped.
  function loadUser() {
    var out = [];
    if (!app.settings.haveSetting(SECTION, KEY)) return out;
    var doc;
    try {
      doc = JSON.parse(app.settings.getSetting(SECTION, KEY));
    } catch (e) {
      return out;
    }
    var list = doc && doc.presets instanceof Array ? doc.presets : [];
    for (var i = 0; i < list.length; i++) {
      var name = cleanName(list[i] && list[i].name);
      var c = sanitized(list[i] && list[i].curve);
      if (name && c && find(BUILT_IN, name) < 0 && find(out, name) < 0) out.push({ name: name, curve: c });
    }
    return out;
  }

  function saveUser(list) {
    app.settings.saveSetting(SECTION, KEY, JSON.stringify({ version: 1, presets: list }));
  }

  // ------------------------------------------------------------------ keyframes

  // Selected properties of the active comp whose keys can ease: {prop, keys} (their selected keys).
  function selectedKeyed() {
    var comp = app.project.activeItem;
    var out = [];
    if (!(comp instanceof CompItem)) return out;
    var props = comp.selectedProperties;
    for (var i = 0; i < props.length; i++) {
      var p = props[i];
      if (p.propertyType !== PropertyType.PROPERTY || p.numKeys < 2 || !p.isInterpolationTypeValid(BEZIER)) continue;
      var keys = p.selectedKeys;
      if (keys.length) out.push({ prop: p, keys: keys });
    }
    return out;
  }

  // Pairs of neighbouring selected keyframes: {prop, key} (the earlier key's index), by property.
  function selectedPairs() {
    var keyed = selectedKeyed(), out = [];
    for (var i = 0; i < keyed.length; i++) {
      var keys = keyed[i].keys;
      for (var k = 0; k + 1 < keys.length; k++) if (keys[k + 1] === keys[k] + 1) out.push({ prop: keyed[i].prop, key: keys[k] });
    }
    return out;
  }

  // Length of the motion path from `a` to `b` with the tangents of their keys.
  function pathLength(a, outTangent, inTangent, b) {
    var p = [], n = Math.max(a.length, b.length), len = 0, prev = null;
    for (var d = 0; d < n; d++) p.push([a[d] || 0, (a[d] || 0) + (outTangent[d] || 0), (b[d] || 0) + (inTangent[d] || 0), b[d] || 0]);
    for (var s = 0; s <= 128; s++) {
      var t = s / 128, u = 1 - t, pt = [];
      for (var e = 0; e < n; e++) pt.push(u * u * u * p[e][0] + 3 * u * u * t * p[e][1] + 3 * u * t * t * p[e][2] + t * t * t * p[e][3]);
      if (prev) {
        var sq = 0;
        for (var f = 0; f < n; f++) sq += (pt[f] - prev[f]) * (pt[f] - prev[f]);
        len += Math.sqrt(sq);
      }
      prev = pt;
    }
    return len;
  }

  // The layer a property belongs to.
  function layerOf(p) {
    return p.propertyGroup(p.propertyDepth);
  }

  // Average speed of segment `a` → `a + 1` for each entry of its eases, in the units the eases use
  // (keyframe times move with the layer, so the duration is in the layer's time). Null when the
  // segment has no duration.
  function slopes(p, a) {
    var stretch = layerOf(p).stretch || 100;
    var dur = (p.keyTime(a + 1) - p.keyTime(a)) * 100 / stretch;
    if (!(dur > 0) || !isFinite(dur)) return null;
    var n = Math.max(1, p.keyOutTemporalEase(a).length);
    var v0 = p.keyValue(a), v1 = p.keyValue(a + 1), out = [];
    var vt = p.propertyValueType;
    if (vt === PropertyValueType.TwoD_SPATIAL || vt === PropertyValueType.ThreeD_SPATIAL) {
      var speed = pathLength(v0, p.keyOutSpatialTangent(a), p.keyInSpatialTangent(a + 1), v1) / dur;
      for (var i = 0; i < n; i++) out.push(speed);
    } else if (typeof v0 === "number" && typeof v1 === "number") {
      out.push((v1 - v0) / dur);
    } else if (v0 instanceof Array && v1 instanceof Array && v0.length === v1.length) {
      for (var d = 0; d < n; d++) out.push(((v1[d] || 0) - (v0[d] || 0)) / dur);
    } else {
      // Shapes and other values ease their overall progress.
      for (var j = 0; j < n; j++) out.push(1 / dur);
    }
    return out;
  }

  // Ease one side of key `k` with `eases`, leaving its other side as it plays (auto-Bezier and
  // linear sides keep their shape; a continuous key stops being continuous, its sides now differ).
  function easeSide(p, k, out, eases) {
    var inType = p.keyInInterpolationType(k), outType = p.keyOutInterpolationType(k);
    var inEase = p.keyInTemporalEase(k), outEase = p.keyOutTemporalEase(k);
    if (p.keyTemporalContinuous(k)) p.setTemporalContinuousAtKey(k, false);
    if (out) {
      p.setTemporalEaseAtKey(k, inEase, eases);
      p.setInterpolationTypeAtKey(k, inType, BEZIER);
    } else {
      p.setTemporalEaseAtKey(k, eases, outEase);
      p.setInterpolationTypeAtKey(k, BEZIER, outType);
    }
  }

  // The eases of one side with `speed` (relative to the segment's average) and `influence`.
  function eases(s, speed, influence) {
    var out = [];
    for (var d = 0; d < s.length; d++) out.push(new KeyframeEase(speed * s[d], influence));
    return out;
  }

  // Ease the sides `mode` names ("In", "Out" or "In-Out") of every selected keyframe with `c`, in
  // one undo step: a side with a neighbouring key, scaled to the segment on that side. Returns
  // {eased, roving}: the number of keyframes eased, and of selected roving keyframes skipped.
  function applyCurve(c, mode) {
    var keyed = selectedKeyed(), sides = [], keys = 0, roving = 0;
    var doIn = mode !== "Out", doOut = mode !== "In";
    // Every side and its eases first: the segments are measured before any key changes.
    for (var i = 0; i < keyed.length; i++) {
      var p = keyed[i].prop;
      for (var j = 0; j < keyed[i].keys.length; j++) {
        var k = keyed[i].keys[j], before = sides.length;
        if (p.keyRoving(k)) {
          roving++;
          continue;
        }
        var s = doIn && k > 1 ? slopes(p, k - 1) : null;
        if (s) sides.push({ prop: p, key: k, out: false, eases: eases(s, c.inSpeed, c.inInfluence) });
        s = doOut && k < p.numKeys ? slopes(p, k) : null;
        if (s) sides.push({ prop: p, key: k, out: true, eases: eases(s, c.outSpeed, c.outInfluence) });
        if (sides.length > before) keys++;
      }
    }
    if (!sides.length) return { eased: 0, roving: roving };
    app.beginUndoGroup("Apply Ease Preset");
    try {
      for (var e = 0; e < sides.length; e++) easeSide(sides[e].prop, sides[e].key, sides[e].out, sides[e].eases);
    } finally {
      app.endUndoGroup();
    }
    return { eased: keys, roving: roving };
  }

  // The curve between the first selected pair of keyframes (measured on the dimension that
  // changes most); null for holds and segments whose value doesn't change.
  function captureCurve() {
    var pairs = selectedPairs();
    if (!pairs.length) return null;
    var p = pairs[0].prop, a = pairs[0].key;
    if (p.keyOutInterpolationType(a) === HOLD) return null;
    var s = slopes(p, a);
    if (!s) return null;
    var d = 0;
    for (var i = 1; i < s.length; i++) if (Math.abs(s[i]) > Math.abs(s[d])) d = i;
    if (Math.abs(s[d]) < 1e-12) return null;
    var o = p.keyOutTemporalEase(a), n = p.keyInTemporalEase(a + 1);
    var oe = o[d] || o[0], ne = n[d] || n[0];
    if (!oe || !ne) return null;
    return sanitized(curve(oe.influence, oe.speed / s[d], ne.influence, ne.speed / s[d]));
  }

  // ------------------------------------------------------------------ panel

  var ui = thisObj instanceof Panel ? thisObj : new Window("palette", "Ease Presets", undefined, { resizeable: true });
  ui.orientation = "column";
  ui.alignChildren = ["fill", "top"];
  ui.spacing = 6;

  var user = loadUser();
  var working = BUILT_IN[0].curve;
  var refreshing = false;

  var list = ui.add("listbox", undefined, [], { name: "presets" });
  list.preferredSize = [220, 150];
  list.helpTip = "Click a preset to apply it to the selected keyframes";

  // The working curve in the normalised value graph: keys at (0, 0) and (1, 1).
  var graph = ui.add("group");
  graph.preferredSize = [220, 110];
  graph.onDraw = function () {
    var g = this.graphics, w = this.size.width, h = this.size.height;
    var lo = -0.35, hi = 1.35, m = 10;
    var at = function (x, v) { return [m + (w - 2 * m) * x, h - m - (h - 2 * m) * (v - lo) / (hi - lo)]; };
    var frame = g.newPen(g.PenType.SOLID_COLOR, [0.5, 0.5, 0.5, 0.6], 1);
    g.newPath();
    g.rectPath(0, 0, w, h);
    g.strokePath(frame);
    g.newPath();
    var a = at(0, 0), b = at(1, 0), c0 = at(0, 1), d = at(1, 1);
    g.moveTo(a[0], a[1]);
    g.lineTo(b[0], b[1]);
    g.moveTo(c0[0], c0[1]);
    g.lineTo(d[0], d[1]);
    g.strokePath(frame);
    // Handles: (out influence, out speed × out influence) and (1 − in influence, 1 − in speed × in influence).
    var oi = working.outInfluence / 100, ii = working.inInfluence / 100;
    var h1 = [oi, working.outSpeed * oi], h2 = [1 - ii, 1 - working.inSpeed * ii];
    var handles = g.newPen(g.PenType.SOLID_COLOR, [0.8, 0.8, 0.8, 1], 1);
    var p1 = at(h1[0], h1[1]), p2 = at(h2[0], h2[1]), k0 = at(0, 0), k1 = at(1, 1);
    g.newPath();
    g.moveTo(k0[0], k0[1]);
    g.lineTo(p1[0], p1[1]);
    g.moveTo(k1[0], k1[1]);
    g.lineTo(p2[0], p2[1]);
    g.strokePath(handles);
    var accent = g.newPen(g.PenType.SOLID_COLOR, [0.29, 0.56, 1, 1], 2);
    g.newPath();
    for (var s = 0; s <= 48; s++) {
      var t = s / 48, u = 1 - t;
      var x = 3 * u * u * t * h1[0] + 3 * u * t * t * h2[0] + t * t * t;
      var v = 3 * u * u * t * h1[1] + 3 * u * t * t * h2[1] + t * t * t;
      var q = at(x, v);
      if (s === 0) g.moveTo(q[0], q[1]);
      else g.lineTo(q[0], q[1]);
    }
    g.strokePath(accent);
  };

  function field(row, label, name) {
    row.add("statictext", undefined, label);
    var f = row.add("edittext", undefined, "", { name: name });
    f.characters = 5;
    return f;
  }
  var outRow = ui.add("group");
  var outInfluence = field(outRow, "Out %", "outInfluence");
  var outSpeed = field(outRow, "Speed", "outSpeed");
  var inRow = ui.add("group");
  var inInfluence = field(inRow, "In %", "inInfluence");
  var inSpeed = field(inRow, "Speed", "inSpeed");

  var modeRow = ui.add("group");
  modeRow.add("statictext", undefined, "Ease");
  var MODES = ["In", "Out", "In-Out"];
  var mode = modeRow.add("dropdownlist", undefined, MODES, { name: "mode" });
  mode.selection = 2;
  mode.helpTip = "The sides of each selected keyframe to ease: incoming, outgoing or both";

  var actions = ui.add("group");
  var applyButton = actions.add("button", undefined, "Apply", { name: "apply" });
  var fromKeys = actions.add("button", undefined, "From Keys", { name: "fromKeys" });
  var nameRow = ui.add("group");
  nameRow.alignChildren = ["fill", "center"];
  var nameField = nameRow.add("edittext", undefined, "", { name: "name" });
  nameField.characters = 14;
  nameField.helpTip = "Preset name";
  var saveButton = nameRow.add("button", undefined, "Save", { name: "save" });
  var edits = ui.add("group");
  var renameButton = edits.add("button", undefined, "Rename", { name: "rename" });
  var deleteButton = edits.add("button", undefined, "Delete", { name: "delete" });
  var status = ui.add("statictext", undefined, "", { name: "status" });
  status.alignment = ["fill", "top"];

  function round(v, places) {
    var f = Math.pow(10, places);
    return String(Math.round(v * f) / f);
  }

  function showCurve(c) {
    working = c;
    outInfluence.text = round(c.outInfluence, 1);
    outSpeed.text = round(c.outSpeed, 2);
    inInfluence.text = round(c.inInfluence, 1);
    inSpeed.text = round(c.inSpeed, 2);
  }

  // The curve typed into the fields (null when one isn't a number).
  function typedCurve() {
    return sanitized(curve(parseFloat(outInfluence.text), parseFloat(outSpeed.text), parseFloat(inInfluence.text), parseFloat(inSpeed.text)));
  }

  function presets() {
    return BUILT_IN.concat(user);
  }

  // The user preset selected in the list (null for built-in ones and no selection).
  function selectedUser() {
    var item = list.selection;
    if (!item) return null;
    var i = find(user, item.text);
    return i < 0 ? null : user[i];
  }

  function refresh(select) {
    refreshing = true;
    list.removeAll();
    var all = presets();
    for (var i = 0; i < all.length; i++) list.add("item", all[i].name);
    if (select) {
      var j = find(all, select);
      if (j >= 0) list.selection = j;
    }
    refreshing = false;
  }

  function apply(c, what) {
    var m = mode.selection ? mode.selection.text : "In-Out";
    var r = applyCurve(c, m), n = r.eased;
    var side = m === "In" ? "before it" : m === "Out" ? "after it" : "next to it";
    var skipped = r.roving > 0 ? "skipped " + r.roving + " roving keyframe(s); turn off Roving to ease them" : "";
    if (n > 0) status.text = "Eased " + n + " keyframe(s) (" + m + ") with " + what + (skipped ? "; " + skipped : "");
    else status.text = skipped ? "Nothing eased: " + skipped : "Select keyframes with another keyframe " + side;
    return n;
  }

  list.onChange = function () {
    if (refreshing || !list.selection) return;
    var all = presets(), i = find(all, list.selection.text);
    if (i < 0) return;
    showCurve(all[i].curve);
    nameField.text = find(user, all[i].name) >= 0 ? all[i].name : "";
    apply(all[i].curve, "“" + all[i].name + "”");
  };

  var onTyped = function () {
    var c = typedCurve();
    if (c) working = c;
  };
  outInfluence.onChange = outSpeed.onChange = inInfluence.onChange = inSpeed.onChange = onTyped;

  applyButton.onClick = function () {
    var c = typedCurve();
    if (!c) {
      status.text = "Type numbers for the influences and speeds";
      return;
    }
    showCurve(c);
    apply(c, "the curve");
  };

  fromKeys.onClick = function () {
    var c = captureCurve();
    if (!c) {
      status.text = "Select two neighbouring keyframes whose value changes";
      return;
    }
    showCurve(c);
    status.text = "Read the curve of the selected keyframes";
  };

  saveButton.onClick = function () {
    var name = cleanName(nameField.text), c = typedCurve();
    if (!name) {
      status.text = "Type a name for the preset first";
      return;
    }
    if (find(BUILT_IN, name) >= 0) {
      status.text = "“" + name + "” is a built-in preset; choose another name";
      return;
    }
    if (!c) {
      status.text = "Type numbers for the influences and speeds";
      return;
    }
    // Saving under an existing name replaces that preset.
    var i = find(user, name);
    if (i >= 0) user[i] = { name: name, curve: c };
    else user.push({ name: name, curve: c });
    saveUser(user);
    refresh(name);
    status.text = "Saved the ease preset “" + name + "”";
  };

  renameButton.onClick = function () {
    var p = selectedUser(), name = cleanName(nameField.text);
    if (!p) {
      status.text = "Select one of your own presets first";
      return;
    }
    if (!name) {
      status.text = "Type a new name for “" + p.name + "” first";
      return;
    }
    var other = find(user, name);
    if (find(BUILT_IN, name) >= 0 || (other >= 0 && user[other] !== p)) {
      status.text = "An ease preset named “" + name + "” exists";
      return;
    }
    p.name = name;
    saveUser(user);
    refresh(name);
    status.text = "Renamed to “" + name + "”";
  };

  deleteButton.onClick = function () {
    var p = selectedUser();
    if (!p) {
      status.text = "Select one of your own presets first";
      return;
    }
    user.splice(find(user, p.name), 1);
    saveUser(user);
    refresh(null);
    nameField.text = "";
    status.text = "Deleted “" + p.name + "”";
  };

  refresh(null);
  showCurve(working);
  status.text = "Select keyframes, then click a preset";
  if (ui instanceof Window) {
    ui.show();
  } else {
    ui.layout.layout(true);
  }
})(this);
