// EffectCraft scripting: an After Effects-style object model over the engine.
//
// Written from the behaviour the After Effects Scripting Guide documents (object names,
// attributes, methods). Reads go through `__query(kind, json)`; every edit runs an engine command
// through `__exec(id, json)`, so it is undoable and identical to the UI's action.

var __ecGlobal = this;

function __call(id, p) {
  var r = __exec(id, JSON.stringify(p || {}));
  return r === undefined || r === "" ? null : JSON.parse(r);
}
function __get(kind, a) {
  return JSON.parse(__query(kind, JSON.stringify(a || {})));
}
function __err(msg) {
  return new Error(msg);
}
function __num(v, what) {
  var n = Number(v);
  if (typeof v === "boolean" || isNaN(n)) throw __err((what || "value") + " must be a number");
  return n;
}
function __str(v) {
  if (v === undefined || v === null) return "";
  if (v instanceof File || v instanceof Folder) return v.fsName;
  return String(v);
}

// ------------------------------------------------------------------ enums

function __enum(name, keys, base) {
  var e = {};
  for (var i = 0; i < keys.length; i++) e[keys[i]] = base + i;
  e.__name = name;
  e.__keys = keys;
  e.__base = base;
  return e;
}
function __enumKey(e, v) {
  var i = v - e.__base;
  return i >= 0 && i < e.__keys.length ? e.__keys[i] : null;
}

var BlendingMode = __enum("BlendingMode", [
  "NORMAL", "DISSOLVE", "DANCING_DISSOLVE", "DARKEN", "MULTIPLY", "COLOR_BURN", "CLASSIC_COLOR_BURN", "LINEAR_BURN",
  "DARKER_COLOR", "ADD", "LIGHTEN", "SCREEN", "COLOR_DODGE", "CLASSIC_COLOR_DODGE", "LINEAR_DODGE", "LIGHTER_COLOR",
  "OVERLAY", "SOFT_LIGHT", "HARD_LIGHT", "LINEAR_LIGHT", "VIVID_LIGHT", "PIN_LIGHT", "HARD_MIX", "DIFFERENCE",
  "CLASSIC_DIFFERENCE", "EXCLUSION", "SUBTRACT", "DIVIDE", "HUE", "SATURATION", "COLOR", "LUMINOSITY", "STENCIL_ALPHA",
  "STENCIL_LUMA", "SILHOUETE_ALPHA", "SILHOUETTE_LUMA", "ALPHA_ADD", "LUMINESCENT_PREMUL"], 5212);
BlendingMode.SILHOUETTE_ALPHA = BlendingMode.SILHOUETE_ALPHA;
var KeyframeInterpolationType = __enum("KeyframeInterpolationType", ["LINEAR", "BEZIER", "HOLD"], 6612);
var TrackMatteType = __enum("TrackMatteType", ["NO_TRACK_MATTE", "ALPHA", "ALPHA_INVERTED", "LUMA", "LUMA_INVERTED"], 5012);
var LightType = __enum("LightType", ["PARALLEL", "SPOT", "POINT", "AMBIENT", "ENVIRONMENT"], 4412);
var ParagraphJustification = __enum("ParagraphJustification", [
  "LEFT_JUSTIFY", "CENTER_JUSTIFY", "RIGHT_JUSTIFY", "FULL_JUSTIFY_LASTLINE_LEFT", "FULL_JUSTIFY_LASTLINE_CENTER",
  "FULL_JUSTIFY_LASTLINE_RIGHT", "FULL_JUSTIFY_LASTLINE_FULL", "MULTIPLE_JUSTIFICATIONS"], 7413);
var PropertyValueType = __enum("PropertyValueType", [
  "NO_VALUE", "ThreeD_SPATIAL", "ThreeD", "TwoD_SPATIAL", "TwoD", "OneD", "COLOR", "CUSTOM_VALUE", "MARKER",
  "LAYER_INDEX", "MASK_INDEX", "SHAPE", "TEXT_DOCUMENT"], 6412);
var PropertyType = __enum("PropertyType", ["PROPERTY", "INDEXED_GROUP", "NAMED_GROUP"], 6212);
var LayerQuality = __enum("LayerQuality", ["BEST", "DRAFT", "WIREFRAME"], 4612);
var LayerSamplingQuality = __enum("LayerSamplingQuality", ["BILINEAR", "BICUBIC"], 4812);
var FrameBlendingType = __enum("FrameBlendingType", ["NO_FRAME_BLEND", "FRAME_MIX", "PIXEL_MOTION"], 4012);
var AutoOrientType = __enum("AutoOrientType", ["NO_AUTO_ORIENT", "ALONG_PATH", "CAMERA_OR_POINT_OF_INTEREST", "CHARACTERS_TOWARD_CAMERA"], 4212);
var MaskMode = __enum("MaskMode", ["NONE", "ADD", "SUBTRACT", "INTERSECT", "LIGHTEN", "DARKEN", "DIFFERENCE"], 6812);
var RQItemStatus = __enum("RQItemStatus", ["WILL_CONTINUE", "NEEDS_OUTPUT", "UNQUEUED", "QUEUED", "RENDERING", "USER_STOPPED", "ERR_STOPPED", "DONE"], 3012);
var CloseOptions = __enum("CloseOptions", ["DO_NOT_SAVE_CHANGES", "PROMPT_TO_SAVE_CHANGES", "SAVE_CHANGES"], 1212);
var ImportAsType = __enum("ImportAsType", ["COMP_CROPPED_LAYERS", "FOOTAGE", "COMP", "PROJECT"], 3812);
var TimeDisplayType = __enum("TimeDisplayType", ["FRAMES", "TIMECODE"], 2012);
var LogType = __enum("LogType", ["ERRORS_ONLY", "ERRORS_AND_SETTINGS", "ERRORS_AND_PER_FRAME_INFO"], 3212);
var KeyframeLabel = __enum("KeyframeLabel", ["NONE"], 0);

var __blendNames = {};
(function () {
  var labels = ["Normal", "Dissolve", "Dancing Dissolve", "Darken", "Multiply", "Color Burn", "Classic Color Burn",
    "Linear Burn", "Darker Color", "Add", "Lighten", "Screen", "Color Dodge", "Classic Color Dodge", "Linear Dodge",
    "Lighter Color", "Overlay", "Soft Light", "Hard Light", "Linear Light", "Vivid Light", "Pin Light", "Hard Mix",
    "Difference", "Classic Difference", "Exclusion", "Subtract", "Divide", "Hue", "Saturation", "Color", "Luminosity",
    "Stencil Alpha", "Stencil Luma", "Silhouette Alpha", "Silhouette Luma", "Alpha Add", "Luminescent Premul"];
  for (var i = 0; i < labels.length; i++) __blendNames[labels[i]] = BlendingMode.__base + i;
})();
function __blendLabel(v) {
  for (var k in __blendNames) if (__blendNames[k] === v) return k;
  throw __err("Bad blendingMode value " + v);
}

var __interpKey = { 6612: "linear", 6613: "bezier", 6614: "hold" };
var __matteKind = { 5013: "alpha", 5014: "alphaInverted", 5015: "luma", 5016: "lumaInverted" };
var __matteEnum = { Alpha: 5013, AlphaInverted: 5014, Luma: 5015, LumaInverted: 5016 };
var __justKey = { 7413: "left", 7414: "center", 7415: "right", 7416: "justifyLeft", 7417: "justifyCenter", 7418: "justifyRight", 7419: "justifyAll" };

// ------------------------------------------------------------------ output, $ and dialogs

function writeLn(s) { __print(__str(s)); }
function write(s) { __print(__str(s)); }
function alert(s) { __print(__str(s)); }
function confirm(s) { __print(__str(s)); return true; }
function prompt(msg, def) { __print(__str(msg)); return def === undefined ? "" : def; }
function clearOutput() {}

var $ = {
  writeln: function () { __print(Array.prototype.slice.call(arguments).map(__str).join("")); },
  write: function () { __print(Array.prototype.slice.call(arguments).map(__str).join("")); },
  sleep: function () {},
  gc: function () {},
  global: __ecGlobal,
  level: 0,
  engineName: "boa",
  get fileName() { return __get("app").scriptName; },
  get os() { return "EffectCraft"; },
  get version() { return "EffectCraft scripting"; },
  get locale() { return "en_US"; },
  evalFile: function () { throw __err("$.evalFile is not supported: scripts can't run scripts"); },
};

// ------------------------------------------------------------------ File / Folder

function __join(a, b) { return a.replace(/[\/\\]+$/, "") + "/" + b; }

function File(path) {
  if (!(this instanceof File)) return new File(path);
  this.__path = path === undefined ? "" : __str(path);
  this.__buf = null;
  this.__mode = null;
  this.__pos = 0;
  this.encoding = "UTF-8";
  this.lineFeed = "Unix";
  this.error = "";
}
File.prototype = {
  constructor: File,
  get fsName() { return this.__path; },
  get fullName() { return this.__path; },
  get absoluteURI() { return this.__path; },
  get name() { return this.__path.split(/[\/\\]/).pop(); },
  get displayName() { return this.name; },
  get path() { var p = this.__path.split(/[\/\\]/); p.pop(); return p.join("/"); },
  get parent() { return new Folder(this.path); },
  get exists() { return JSON.parse(__file("exists", this.__path)) === true; },
  get length() { return this.__buf !== null ? this.__buf.length : (this.exists ? JSON.parse(__file("read", this.__path)).length : 0); },
  get eof() { return this.__buf === null || this.__pos >= this.__buf.length; },
  toString: function () { return this.__path; },
  open: function (mode) {
    mode = (mode || "r").charAt(0).toLowerCase();
    this.__mode = mode;
    this.__pos = 0;
    if (mode === "r" || mode === "e") {
      // Throws when the location is off limits (the scripting file gate).
      if (!JSON.parse(__file("access", this.__path))) {
        if (mode === "r") { this.error = "File not found"; return false; }
        this.__buf = "";
        return true;
      }
      this.__buf = JSON.parse(__file("read", this.__path));
    } else if (mode === "a") {
      this.__buf = this.exists ? JSON.parse(__file("read", this.__path)) : "";
      this.__pos = this.__buf.length;
    } else {
      // Check the gate now, like After Effects refusing to open a file for writing.
      JSON.parse(__file("write", this.__path, ""));
      this.__buf = "";
    }
    return true;
  },
  read: function (n) {
    if (this.__buf === null) this.open("r");
    if (this.__buf === null) return "";
    var s = n === undefined ? this.__buf.substring(this.__pos) : this.__buf.substring(this.__pos, this.__pos + n);
    this.__pos += s.length;
    return s;
  },
  readln: function () {
    if (this.__buf === null) this.open("r");
    if (this.__buf === null) return "";
    var i = this.__buf.indexOf("\n", this.__pos);
    var end = i < 0 ? this.__buf.length : i;
    var s = this.__buf.substring(this.__pos, end).replace(/\r$/, "");
    this.__pos = i < 0 ? this.__buf.length : i + 1;
    return s;
  },
  write: function () {
    if (this.__mode !== "w" && this.__mode !== "a" && this.__mode !== "e") throw __err("File is not open for writing");
    var s = Array.prototype.slice.call(arguments).map(__str).join("");
    this.__buf = this.__buf.substring(0, this.__pos) + s + this.__buf.substring(this.__pos + s.length);
    this.__pos += s.length;
    return true;
  },
  writeln: function () { return this.write(Array.prototype.slice.call(arguments).map(__str).join("") + "\n"); },
  seek: function (p) { this.__pos = p; return true; },
  tell: function () { return this.__pos; },
  close: function () {
    if ((this.__mode === "w" || this.__mode === "a" || this.__mode === "e") && this.__buf !== null) {
      __file("write", this.__path, this.__buf);
    }
    this.__buf = null;
    this.__mode = null;
    return true;
  },
  remove: function () { return JSON.parse(__file("remove", this.__path)); },
  copy: function (to) {
    var data = JSON.parse(__file("read", this.__path));
    __file("write", __str(to), data);
    return true;
  },
  rename: function () { throw __err("File.rename is not supported"); },
  execute: function () { return JSON.parse(__file("execute", this.__path)); },
  openDlg: function () { return null; },
  saveDlg: function () { return null; },
};
File.openDialog = function () { return null; };
File.saveDialog = function () { return null; };
File.decode = function (s) { return decodeURIComponent(s); };
File.encode = function (s) { return encodeURIComponent(s); };
File.fs = "EffectCraft";

function Folder(path) {
  if (!(this instanceof Folder)) return new Folder(path);
  this.__path = path === undefined ? "" : __str(path);
}
Folder.prototype = {
  constructor: Folder,
  get fsName() { return this.__path; },
  get fullName() { return this.__path; },
  get absoluteURI() { return this.__path; },
  get name() { return this.__path.replace(/[\/\\]+$/, "").split(/[\/\\]/).pop(); },
  get displayName() { return this.name; },
  get path() { var p = this.__path.replace(/[\/\\]+$/, "").split(/[\/\\]/); p.pop(); return p.join("/"); },
  get parent() { return new Folder(this.path); },
  get exists() { return JSON.parse(__file("isDir", this.__path)) === true; },
  toString: function () { return this.__path; },
  create: function () { return JSON.parse(__file("mkdir", this.__path)); },
  remove: function () { return JSON.parse(__file("remove", this.__path)); },
  getFiles: function (mask) {
    var names = JSON.parse(__file("list", this.__path));
    var out = [];
    for (var i = 0; i < names.length; i++) {
      var n = names[i];
      if (typeof mask === "string" && mask !== "*") {
        var re = new RegExp("^" + mask.replace(/[.+^${}()|[\]\\]/g, "\\$&").replace(/\*/g, ".*").replace(/\?/g, ".") + "$", "i");
        if (!re.test(n.split(/[\/\\]/).pop())) continue;
      }
      var isDir = JSON.parse(__file("isDir", n));
      var f = isDir ? new Folder(n) : new File(n);
      if (typeof mask === "function" && !mask(f)) continue;
      out.push(f);
    }
    return out;
  },
  selectDlg: function () { return null; },
};
Folder.selectDialog = function () { return null; };
Object.defineProperty(Folder, "current", { get: function () { var d = __get("app").projectDir; return d ? new Folder(d) : null; } });
Object.defineProperty(Folder, "temp", { get: function () { return null; } });
Object.defineProperty(Folder, "desktop", { get: function () { return null; } });
Object.defineProperty(Folder, "myDocuments", { get: function () { return null; } });

// TCP sockets (behind Allow Scripts to Write Files and Access Network; see socket.rs).
function Socket() {
  if (!(this instanceof Socket)) return new Socket();
  __sock("check", 0, "{}");
  this.__id = 0;
  this.host = "";
  this.timeout = 10;
  this.encoding = "ASCII";
  this.error = "";
}
function __sockCall(s, op, extra) {
  var p = extra || {};
  p.timeout = s.timeout;
  p.encoding = s.encoding;
  var r = JSON.parse(__sock(op, s.__id, JSON.stringify(p)));
  s.error = r.error || "";
  return r;
}
Socket.prototype = {
  constructor: Socket,
  get connected() { return this.__id > 0 && __sockCall(this, "connected").value === true; },
  get eof() { return this.__id === 0 || __sockCall(this, "eof").value === true; },
  open: function (host, encoding) {
    if (this.__id) this.close();
    if (encoding) this.encoding = String(encoding);
    var r = __sockCall(this, "open", { host: String(host) });
    if (r.error) return false;
    this.__id = r.id;
    this.host = String(host);
    return true;
  },
  listen: function (port, encoding) {
    if (this.__id) this.close();
    if (encoding) this.encoding = String(encoding);
    var r = __sockCall(this, "listen", { port: __num(port, "port") });
    if (r.error) return false;
    this.__id = r.id;
    this.host = "127.0.0.1:" + r.port;
    this.port = r.port;
    return true;
  },
  poll: function () {
    if (!this.__id) return null;
    var r = __sockCall(this, "poll");
    if (r.id === null || r.id === undefined) return null;
    var s = Object.create(Socket.prototype);
    s.__id = r.id;
    s.host = r.host || "";
    s.timeout = this.timeout;
    s.encoding = this.encoding;
    s.error = "";
    return s;
  },
  read: function (count) {
    if (!this.__id) return "";
    return __sockCall(this, "read", { count: count === undefined ? -1 : __num(count, "count") }).data || "";
  },
  readln: function () { return this.__id ? __sockCall(this, "readln").data || "" : ""; },
  write: function () {
    if (!this.__id) return false;
    var s = Array.prototype.slice.call(arguments).map(__str).join("");
    return !__sockCall(this, "write", { data: s }).error;
  },
  writeln: function () { return this.write(Array.prototype.slice.call(arguments).map(__str).join("") + "\n"); },
  close: function () {
    if (this.__id) __sockCall(this, "close");
    this.__id = 0;
    return true;
  },
  toString: function () { return "[object Socket]"; }
};
var system = { callSystem: function () { throw __err("scripts can't run system commands"); } };

// ------------------------------------------------------------------ helpers

// Unknown attributes of property groups and layers resolve to child properties, like After
// Effects (`layer.transform.position`, `layer.Effects`, `effect.blurriness`).
var __attr = {
  get: function (t, k, r) {
    if (typeof k !== "string" || k in t || k.charAt(0) === "_" || k === "toJSON" || k === "then") return Reflect.get(t, k, r);
    var c = t.property(k);
    return c === null ? undefined : c;
  },
};

// 1-based collections with `[n]` access.
var __coll = {
  get: function (t, k, r) {
    if (typeof k === "string" && /^[0-9]+$/.test(k)) return t.__at(parseInt(k, 10));
    return Reflect.get(t, k, r);
  },
};

function KeyframeEase(speed, influence) {
  if (!(this instanceof KeyframeEase)) return new KeyframeEase(speed, influence);
  this.speed = speed === undefined ? 0 : speed;
  this.influence = influence === undefined ? 16.666666667 : influence;
}

function Shape() {
  if (!(this instanceof Shape)) return new Shape();
  this.vertices = [];
  this.inTangents = [];
  this.outTangents = [];
  this.closed = true;
  this.featherSegLocs = [];
  this.featherRelSegLocs = [];
  this.featherRadii = [];
  this.featherInterps = [];
  this.featherTensions = [];
  this.featherTypes = [];
  this.featherRelCornerAngles = [];
}

function MarkerValue(comment, chapter, url, frameTarget, cuePointName, params) {
  if (!(this instanceof MarkerValue)) return new MarkerValue(comment, chapter, url, frameTarget, cuePointName, params);
  this.comment = comment === undefined ? "" : String(comment);
  this.chapter = chapter === undefined ? "" : String(chapter);
  this.url = url === undefined ? "" : String(url);
  this.frameTarget = frameTarget === undefined ? "" : String(frameTarget);
  this.cuePointName = cuePointName === undefined ? "" : String(cuePointName);
  this.duration = 0;
  this.eventCuePoint = false;
  this.label = 0;
  this.protectedRegion = false;
  this.__params = [];
  if (params) this.setParameters(params);
}
MarkerValue.prototype.getParameters = function () {
  var o = {};
  for (var i = 0; i < this.__params.length; i++) o[this.__params[i][0]] = this.__params[i][1];
  return o;
};
MarkerValue.prototype.setParameters = function (o) {
  this.__params = [];
  for (var k in o) this.__params.push([k, String(o[k])]);
};

// ------------------------------------------------------------------ TextDocument

// `layer.setText` keys of each TextDocument attribute.
var __textKeys = {
  font: "font", fontStyle: "style", fontSize: "size", fillColor: "fill", strokeColor: "stroke", strokeWidth: "strokeWidth",
  applyFill: "applyFill", applyStroke: "applyStroke", strokeOverFill: "strokeOverFill", tracking: "tracking",
  baselineShift: "baselineShift", horizontalScale: "hScale", verticalScale: "vScale", fauxBold: "fauxBold",
  fauxItalic: "fauxItalic", allCaps: "allCaps", smallCaps: "smallCaps", tsume: "tsume", superscript: "superscript",
  subscript: "subscript", ligature: "ligatures", startIndent: "indentLeft", endIndent: "indentRight",
  firstLineIndent: "indentFirst", spaceBefore: "spaceBefore", spaceAfter: "spaceAfter",
};

function TextDocument(text) {
  if (!(this instanceof TextDocument)) return new TextDocument(text);
  this.__base = null;
  this.__changed = {};
  this.__text = text === undefined ? "" : String(text);
  this.__textSet = text !== undefined;
}
TextDocument.__from = function (doc) {
  var t = new TextDocument();
  t.__base = doc;
  t.__text = doc.text;
  t.__textSet = false;
  return t;
};
TextDocument.prototype = {
  constructor: TextDocument,
  get text() { return this.__text; },
  set text(v) { this.__text = String(v); this.__textSet = true; },
  __style: function (k, d) {
    if (k in this.__changed) return this.__changed[k];
    if (this.__base && this.__base.first && k in this.__base.first) return this.__base.first[k];
    if (this.__base && this.__base.para && k in this.__base.para) return this.__base.para[k];
    return d;
  },
  __set: function (k, v) { this.__changed[k] = v; },
  get font() { return this.__style("font", "Inter"); },
  set font(v) { this.__set("font", String(v)); },
  get fontFamily() { return this.font; },
  get fontStyle() { return this.__style("style", "Regular"); },
  set fontStyle(v) { this.__set("style", String(v)); },
  get fontSize() { return this.__style("size", 72); },
  set fontSize(v) { this.__set("size", __num(v, "fontSize")); },
  get fillColor() { var c = this.__style("fill", [1, 1, 1, 1]); return [c[0], c[1], c[2]]; },
  set fillColor(v) { this.__set("fill", [v[0], v[1], v[2], v.length > 3 ? v[3] : 1]); },
  get strokeColor() { var c = this.__style("stroke", [0, 0, 0, 1]); return [c[0], c[1], c[2]]; },
  set strokeColor(v) { this.__set("stroke", [v[0], v[1], v[2], v.length > 3 ? v[3] : 1]); },
  get strokeWidth() { return this.__style("strokeWidth", 0); },
  set strokeWidth(v) { this.__set("strokeWidth", __num(v, "strokeWidth")); },
  get applyFill() { return this.__style("applyFill", true); },
  set applyFill(v) { this.__set("applyFill", !!v); },
  get applyStroke() { return this.__style("applyStroke", false); },
  set applyStroke(v) { this.__set("applyStroke", !!v); },
  get strokeOverFill() { return "strokeOverFill" in this.__changed ? this.__changed.strokeOverFill : (this.__base ? this.__base.stroke_over_fill : true); },
  set strokeOverFill(v) { this.__set("strokeOverFill", !!v); },
  get tracking() { return this.__style("tracking", 0); },
  set tracking(v) { this.__set("tracking", __num(v, "tracking")); },
  get leading() { var l = this.__style("leading", "auto"); return l === "auto" ? this.fontSize * 1.2 : l; },
  set leading(v) { this.__set("leading", __num(v, "leading")); },
  get autoLeading() { return this.__style("leading", "auto") === "auto"; },
  set autoLeading(v) { if (v) this.__set("leading", "auto"); },
  get justification() {
    var j = this.__style("justify", "left");
    for (var k in __justKey) if (__justKey[k] === j) return Number(k);
    return ParagraphJustification.LEFT_JUSTIFY;
  },
  set justification(v) {
    var k = __justKey[v];
    if (!k) throw __err("Bad justification value " + v);
    this.__set("justify", k);
  },
  get baselineShift() { return this.__style("baselineShift", 0); },
  set baselineShift(v) { this.__set("baselineShift", __num(v)); },
  get horizontalScale() { return this.__style("hScale", 100) / 100; },
  set horizontalScale(v) { this.__set("hScale", __num(v) * 100); },
  get verticalScale() { return this.__style("vScale", 100) / 100; },
  set verticalScale(v) { this.__set("vScale", __num(v) * 100); },
  get fauxBold() { return this.__style("fauxBold", false); },
  set fauxBold(v) { this.__set("fauxBold", !!v); },
  get fauxItalic() { return this.__style("fauxItalic", false); },
  set fauxItalic(v) { this.__set("fauxItalic", !!v); },
  get allCaps() { return this.__style("allCaps", false); },
  set allCaps(v) { this.__set("allCaps", !!v); },
  get smallCaps() { return this.__style("smallCaps", false); },
  set smallCaps(v) { this.__set("smallCaps", !!v); },
  get superscript() { return this.__style("baseline", "normal") === "superscript"; },
  set superscript(v) { this.__set("superscript", !!v); },
  get subscript() { return this.__style("baseline", "normal") === "subscript"; },
  set subscript(v) { this.__set("subscript", !!v); },
  get tsume() { return this.__style("tsume", 0); },
  set tsume(v) { this.__set("tsume", __num(v)); },
  get boxText() { return !!(this.__base && this.__base.box_size); },
  get pointText() { return !this.boxText; },
  get boxTextSize() { return this.__base && this.__base.box_size ? this.__base.box_size.slice() : undefined; },
  set boxTextSize(v) {
    var p = this.boxTextPos || [0, 0];
    this.__set("box", [p[0], p[1], v[0], v[1]]);
  },
  get boxTextPos() { return this.__base && this.__base.box_size ? this.__base.box_pos.slice() : undefined; },
  get startIndent() { return this.__style("indentLeft", 0); },
  set startIndent(v) { this.__set("indentLeft", __num(v)); },
  get endIndent() { return this.__style("indentRight", 0); },
  set endIndent(v) { this.__set("indentRight", __num(v)); },
  get firstLineIndent() { return this.__style("indentFirst", 0); },
  set firstLineIndent(v) { this.__set("indentFirst", __num(v)); },
  get spaceBefore() { return this.__style("spaceBefore", 0); },
  set spaceBefore(v) { this.__set("spaceBefore", __num(v)); },
  get spaceAfter() { return this.__style("spaceAfter", 0); },
  set spaceAfter(v) { this.__set("spaceAfter", __num(v)); },
  resetCharStyle: function () {
    var d = { font: "Inter", style: "Regular", size: 72, fill: [1, 1, 1, 1], stroke: [0, 0, 0, 1], strokeWidth: 0, applyFill: true,
      applyStroke: false, tracking: 0, leading: "auto", baselineShift: 0, hScale: 100, vScale: 100, tsume: 0, fauxBold: false,
      fauxItalic: false, allCaps: false, smallCaps: false };
    for (var k in d) this.__changed[k] = d[k];
  },
  resetParagraphStyle: function () {
    var d = { justify: "left", indentLeft: 0, indentRight: 0, indentFirst: 0, spaceBefore: 0, spaceAfter: 0 };
    for (var k in d) this.__changed[k] = d[k];
  },
  toString: function () { return this.__text; },
  // `layer.setText` attributes to apply.
  __attrs: function () {
    var a = {};
    for (var k in this.__changed) a[k] = this.__changed[k];
    if (this.__textSet || !this.__base || this.__text !== this.__base.text) a.text = this.__text;
    return a;
  },
};

// ------------------------------------------------------------------ properties

function __propOf(layer, info) {
  if (!info) return null;
  if (info.isLayer) return layer;
  if (info.kind === "group") return info.isMask ? new MaskPropertyGroup(layer, info.uid) : new PropertyGroup(layer, info.uid);
  return new Property(layer, info.uid);
}

function PropertyBase() {}
PropertyBase.prototype = {
  __info: function () { return __get("node", { comp: this.__layer.__comp, layer: this.__layer.__id, uid: this.__uid }); },
  __ref: function (p) {
    p = p || {};
    p.comp = this.__layer.__comp;
    p.layer = this.__layer.__id;
    p.prop = this.__uid;
    return p;
  },
  get name() { return this.__info().name; },
  set name(v) {
    var i = this.__info();
    if (!i.instance) throw __err("Can not set the name of \"" + i.name + "\": it is not an indexed group member");
    __call("prop.renameGroup", this.__ref({ name: String(v) }));
  },
  get matchName() { return this.__info().matchName; },
  get propertyIndex() { return this.__info().index; },
  get propertyDepth() { return this.__info().depth; },
  get propertyType() { return PropertyType[this.__info().propertyType]; },
  get parentProperty() {
    var i = this.__info();
    if (i.depth <= 1) return this.__layer;
    return __propOf(this.__layer, __get("node", { comp: this.__layer.__comp, layer: this.__layer.__id, uid: i.parent }));
  },
  get isModified() { return !!this.__info().isModified; },
  get canSetEnabled() { return !!this.__info().canSetEnabled; },
  get enabled() { return this.__info().enabled !== false; },
  set enabled(v) {
    if (!this.canSetEnabled) throw __err("Can not set enabled on \"" + this.name + "\"");
    __call("prop.setGroupEnabled", this.__ref({ value: !!v }));
  },
  get active() { return this.enabled && this.__layer.enabled; },
  get elided() { return false; },
  get isEffect() { return !!this.__info().isEffect; },
  get isMask() { return !!this.__info().isMask; },
  get selected() {
    var c = __get("item", { id: this.__layer.__comp }).selectedProperties;
    for (var i = 0; i < c.length; i++) if (c[i][0] === this.__layer.__id && c[i][1] === this.__uid) return true;
    return false;
  },
  set selected(v) {
    if (v) __call("prop.select", { comp: this.__layer.__comp, layer: this.__layer.__id, prop: this.__uid, add: true });
  },
  propertyGroup: function (n) {
    var p = this;
    n = n === undefined ? 1 : n;
    for (var i = 0; i < n && p; i++) p = p.parentProperty;
    return p || null;
  },
  remove: function () { __call("prop.removeGroup", this.__ref()); },
  moveTo: function (i) { __call("prop.moveGroup", this.__ref({ index: __num(i, "index") })); },
  duplicate: function () {
    var r = __call("prop.duplicateGroup", this.__ref());
    return __propOf(this.__layer, __get("node", { comp: this.__layer.__comp, layer: this.__layer.__id, uid: r.prop }));
  },
  toString: function () { return "[object " + this.__kind + "]"; },
};

function PropertyGroup(layer, uid) {
  this.__layer = layer;
  this.__uid = uid;
  this.__kind = "PropertyGroup";
  return new Proxy(this, __attr);
}
PropertyGroup.prototype = Object.create(PropertyBase.prototype);
PropertyGroup.prototype.constructor = PropertyGroup;
Object.defineProperty(PropertyGroup.prototype, "numProperties", { get: function () { return this.__info().numProperties; } });
PropertyGroup.prototype.property = function (key) {
  if (key === undefined || key === null) throw __err("property(): missing name or index");
  var k = typeof key === "number" ? key : String(key);
  return __propOf(this.__layer, __get("child", { comp: this.__layer.__comp, layer: this.__layer.__id, uid: this.__uid, key: k }));
};
PropertyGroup.prototype.canAddProperty = function (name) {
  var r = __get("resolveAdd", { comp: this.__layer.__comp, layer: this.__layer.__id, uid: this.__uid, name: String(name) });
  return !r.error;
};
PropertyGroup.prototype.addProperty = function (name) {
  var L = this.__layer;
  var r = __get("resolveAdd", { comp: L.__comp, layer: L.__id, uid: this.__uid, name: String(name) });
  if (r.error) throw __err(r.error);
  var uid;
  if (r.kind === "effect") uid = __call("effect.apply", { comp: L.__comp, layers: [L.__id], effect: r.effect }).effects[0];
  else if (r.kind === "mask") uid = __call("layer.addMask", { comp: L.__comp, layer: L.__id }).mask;
  else if (r.kind === "shape") {
    var p = { comp: L.__comp, layer: L.__id, kind: r.shape };
    if (r.group !== null && r.group !== undefined) p.group = r.group;
    uid = __call("layer.addShapeItem", p).uid;
  } else if (r.kind === "animator") {
    uid = __call("layer.addTextAnimator", { comp: L.__comp, layer: L.__id, properties: [] }).animator;
  } else if (r.kind === "animatorProperty") {
    var before = this.numProperties;
    __call("layer.addTextAnimatorProperty", { comp: L.__comp, layer: L.__id, animator: r.animator, property: r.property });
    return this.property(before + 1);
  } else if (r.kind === "selector") {
    var n0 = this.numProperties;
    __call("layer.addTextSelector", { comp: L.__comp, layer: L.__id, animator: r.animator, kind: r.selector });
    return this.property(n0 + 1);
  } else throw __err("Can not add \"" + name + "\" here");
  return __propOf(L, __get("node", { comp: L.__comp, layer: L.__id, uid: uid }));
};

function MaskPropertyGroup(layer, uid) {
  this.__layer = layer;
  this.__uid = uid;
  this.__kind = "MaskPropertyGroup";
  return new Proxy(this, __attr);
}
MaskPropertyGroup.prototype = Object.create(PropertyGroup.prototype);
MaskPropertyGroup.prototype.constructor = MaskPropertyGroup;
(function () {
  var modes = { "None": 6812, "Add": 6813, "Subtract": 6814, "Intersect": 6815, "Lighten": 6816, "Darken": 6817, "Difference": 6818 };
  function mask(o) { return { comp: o.__layer.__comp, layer: o.__layer.__id, mask: o.__uid }; }
  Object.defineProperty(MaskPropertyGroup.prototype, "maskMode", {
    get: function () { return modes[this.__info().maskMode]; },
    set: function (v) {
      var p = mask(this);
      for (var k in modes) if (modes[k] === v) p.mode = k;
      if (!p.mode) throw __err("Bad MaskMode value " + v);
      __call("layer.mask.mode", p);
    },
  });
  Object.defineProperty(MaskPropertyGroup.prototype, "inverted", {
    get: function () { return !!this.__info().inverted; },
    set: function (v) { var p = mask(this); p.value = !!v; __call("layer.mask.invert", p); },
  });
  Object.defineProperty(MaskPropertyGroup.prototype, "locked", {
    get: function () { return !!this.__info().locked; },
    set: function (v) { var p = mask(this); p.value = !!v; __call("layer.mask.lock", p); },
  });
  Object.defineProperty(MaskPropertyGroup.prototype, "color", { get: function () { return this.__info().color; } });
  MaskPropertyGroup.prototype.remove = function () { __call("layer.mask.remove", mask(this)); };
})();

function Property(layer, uid) {
  this.__layer = layer;
  this.__uid = uid;
  this.__kind = "Property";
  return new Proxy(this, __attr);
}
Property.prototype = Object.create(PropertyBase.prototype);
Property.prototype.constructor = Property;
Property.prototype.property = function () { return null; };
(function () {
  var P = Property.prototype;
  function def(name, get, set) { Object.defineProperty(P, name, { get: get, set: set }); }
  def("propertyValueType", function () { return PropertyValueType[this.__info().valueType]; });
  def("value", function () { return this.valueAtTime(this.__layer.containingComp.time, false); });
  def("numKeys", function () { return this.__info().numKeys; });
  def("expression", function () { return this.__info().expression; }, function (v) {
    if (!this.canSetExpression) throw __err("Can not set an expression on \"" + this.name + "\"");
    __call("prop.setExpression", this.__ref({ expression: String(v) }));
  });
  def("expressionEnabled", function () { return !!this.__info().expressionEnabled; }, function (v) {
    __call("prop.setExpression", this.__ref({ enabled: !!v }));
  });
  def("expressionError", function () {
    var e = this.expression;
    return e ? __get("exprError", { code: e }) : "";
  });
  def("canSetExpression", function () { return !!this.__info().canSetExpression; });
  def("canVaryOverTime", function () { return !!this.__info().canVaryOverTime; });
  def("isTimeVarying", function () { var i = this.__info(); return i.numKeys > 0 || !!i.expressionEnabled; });
  def("isSpatial", function () { return !!this.__info().spatial; });
  def("unitsText", function () { return this.__info().ui; });
  def("hasMin", function () { return this.__info().min !== undefined; });
  def("hasMax", function () { return this.__info().max !== undefined; });
  def("minValue", function () { var i = this.__info(); if (i.min === undefined) throw __err("property has no minimum"); return i.min; });
  def("maxValue", function () { var i = this.__info(); if (i.max === undefined) throw __err("property has no maximum"); return i.max; });
  def("dimensionsSeparated", function () { return !!this.__info().separated; }, function (v) {
    __call("prop.separateDimensions", { comp: this.__layer.__comp, layer: this.__layer.__id, value: !!v });
  });
  def("isSeparationFollower", function () { return !!this.__info().separationFollower; });
  def("isSeparationLeader", function () { return this.__info().matchId === "position" && this.__info().depth === 2; });
  def("separationDimension", function () { var m = this.__info().matchId; return m === "positionY" ? 1 : m === "positionZ" ? 2 : 0; });
  def("separationLeader", function () { return this.isSeparationFollower ? this.parentProperty.property("ADBE Position") : null; });
  def("selectedKeys", function () {
    var k = this.__keys(), out = [];
    for (var i = 0; i < k.length; i++) if (k[i].selected) out.push(i + 1);
    return out;
  });
  def("isDropdownEffect", function () { return false; });

  // Essential Graphics: expose this property in `comp` (its own composition).
  P.canAddToMotionGraphicsTemplate = function (comp) {
    if (!comp || comp.__id !== this.__layer.__comp) return false;
    return __call("essential.canAdd", this.__ref()).ok === true;
  };
  P.addToMotionGraphicsTemplateAs = function (comp, name) {
    if (!this.canAddToMotionGraphicsTemplate(comp)) return false;
    var p = this.__ref();
    if (name !== undefined && name !== null) p.name = String(name);
    __call("essential.addProperty", p);
    return true;
  };
  P.addToMotionGraphicsTemplate = function (comp) { return this.addToMotionGraphicsTemplateAs(comp); };
  P.getSeparationFollower = function (dim) {
    return this.parentProperty.property(["ADBE Position_0", "ADBE Position_1", "ADBE Position_2"][dim]);
  };
  P.__keys = function () { return __get("keys", { comp: this.__layer.__comp, layer: this.__layer.__id, uid: this.__uid }); };
  P.__key = function (i) {
    var k = this.__keys();
    i = __num(i, "keyIndex");
    if (i < 1 || i > k.length || Math.floor(i) !== i) throw __err("keyIndex " + i + " is out of range 1.." + k.length);
    return k[i - 1];
  };
  // Script value → engine JSON for `prop.set` / `prop.addKey`.
  P.__toEngine = function (v, info, time) {
    var t = info.valueType;
    if (t === "TEXT_DOCUMENT") {
      var td = v instanceof TextDocument ? v : new TextDocument(String(v));
      return __get("textdoc", { comp: this.__layer.__comp, layer: this.__layer.__id, uid: this.__uid, time: time, attrs: td.__attrs() });
    }
    if (t === "SHAPE") {
      if (!v || !v.vertices) throw __err("value must be a Shape");
      var n = v.vertices.length;
      function fit(a) { var o = []; for (var i = 0; i < n; i++) o.push(a && a[i] ? [a[i][0], a[i][1]] : [0, 0]); return o; }
      return { vertices: fit(v.vertices), in_tangents: fit(v.inTangents), out_tangents: fit(v.outTangents), closed: v.closed !== false };
    }
    if (t === "LAYER_INDEX") {
      var idx = v && v.__id !== undefined ? v.index : __num(v);
      if (!idx) return null;
      return this.__layer.containingComp.layer(idx).__id;
    }
    if (info.type === "enum") return __num(v) - 1;
    if (info.type === "bool") return typeof v === "boolean" ? v : __num(v) !== 0;
    if (info.type === "color") {
      if (!(v instanceof Array) || v.length < 3) throw __err("value must be an array of 3 or 4 numbers");
      return [v[0], v[1], v[2], v.length > 3 ? v[3] : 1];
    }
    if (info.dims > 1) {
      if (!(v instanceof Array)) throw __err("value must be an array of " + info.dims + " numbers");
      var o = [];
      for (var d = 0; d < info.dims; d++) o.push(d < v.length ? __num(v[d]) : (d === 2 ? 0 : __num(v[v.length - 1])));
      return o;
    }
    if (v instanceof Array) throw __err("value must be a number, not an array");
    return __num(v);
  };
  P.__fromEngine = function (v, info) {
    if (info.valueType === "TEXT_DOCUMENT") return TextDocument.__from(v);
    if (info.valueType === "SHAPE") {
      var s = new Shape();
      s.vertices = v.vertices;
      s.inTangents = v.inTangents;
      s.outTangents = v.outTangents;
      s.closed = v.closed;
      return s;
    }
    if (info.type === "color" && v instanceof Array) return v.slice();
    return v;
  };
  P.valueAtTime = function (t, preExpression) {
    var info = this.__info();
    var v = __get("value", { comp: this.__layer.__comp, layer: this.__layer.__id, uid: this.__uid, time: __num(t, "time"), pre: !!preExpression });
    return this.__fromEngine(v, info);
  };
  P.setValue = function (v) {
    var info = this.__info();
    if (info.numKeys > 0) throw __err("Can not use setValue on \"" + info.name + "\": it has keyframes (use setValueAtTime or setValueAtKey)");
    if (info.hidden) throw __err("\"" + info.name + "\" can not be set");
    __call("prop.set", this.__ref({ value: this.__toEngine(v, info) }));
  };
  P.setValueAtTime = function (t, v) {
    var info = this.__info();
    t = __num(t, "time");
    __call("prop.addKey", this.__ref({ time: this.__layer.__layerTime(t), value: this.__toEngine(v, info, t) }));
  };
  P.setValuesAtTimes = function (ts, vs) {
    if (!(ts instanceof Array) || !(vs instanceof Array) || ts.length !== vs.length) throw __err("setValuesAtTimes: times and values must be arrays of the same length");
    for (var i = 0; i < ts.length; i++) this.setValueAtTime(ts[i], vs[i]);
  };
  P.setValueAtKey = function (i, v) {
    var k = this.__key(i), info = this.__info();
    __call("keys.set", this.__ref({ time: k.layerTime, value: this.__toEngine(v, info, k.time) }));
  };
  P.addKey = function (t) {
    t = __num(t, "time");
    __call("prop.addKey", this.__ref({ time: this.__layer.__layerTime(t) }));
    return this.nearestKeyIndex(t);
  };
  P.nearestKeyIndex = function (t) {
    var k = this.__keys(), best = 0, d = Infinity;
    if (!k.length) throw __err("\"" + this.name + "\" has no keyframes");
    for (var i = 0; i < k.length; i++) {
      var e = Math.abs(k[i].time - t);
      if (e < d) { d = e; best = i + 1; }
    }
    return best;
  };
  P.keyTime = function (i) { return this.__key(i).time; };
  P.keyValue = function (i) { return this.__fromEngine(this.__key(i).value, this.__info()); };
  // Select key `i` (only) so the selection-based keyframe commands act on it.
  P.__select = function (i) {
    var k = this.__key(i);
    __call("keys.select", { comp: this.__layer.__comp, keys: [{ layer: this.__layer.__id, prop: this.__uid, time: k.layerTime }] });
    return k;
  };
  P.removeKey = function (i) {
    this.__select(i);
    __call("keys.delete", { comp: this.__layer.__comp });
  };
  P.keyInInterpolationType = function (i) { return KeyframeInterpolationType[this.__key(i).inInterp]; };
  P.keyOutInterpolationType = function (i) { return KeyframeInterpolationType[this.__key(i).outInterp]; };
  P.isInterpolationTypeValid = function (t) { return t === 6612 || t === 6613 || t === 6614; };
  P.setInterpolationTypeAtKey = function (i, inType, outType) {
    if (outType === undefined) outType = inType;
    if (!__interpKey[inType] || !__interpKey[outType]) throw __err("Bad KeyframeInterpolationType");
    this.__select(i);
    __call("keys.interpolation", { comp: this.__layer.__comp, "in": __interpKey[inType], out: __interpKey[outType] });
  };
  P.keyInTemporalEase = function (i) { return this.__key(i).inEase.map(function (e) { return new KeyframeEase(e.speed, e.influence); }); };
  P.keyOutTemporalEase = function (i) { return this.__key(i).outEase.map(function (e) { return new KeyframeEase(e.speed, e.influence); }); };
  P.setTemporalEaseAtKey = function (i, inEase, outEase) {
    if (outEase === undefined) outEase = inEase;
    if (!(inEase instanceof Array) || !(outEase instanceof Array)) throw __err("setTemporalEaseAtKey: eases must be arrays of KeyframeEase");
    this.__select(i);
    function sp(a) { return a.map(function (e) { return __num(e.speed, "speed"); }); }
    function inf(a) {
      return a.map(function (e) {
        var x = __num(e.influence, "influence");
        if (x < 0.1 || x > 100) throw __err("influence must be between 0.1 and 100");
        return x;
      });
    }
    __call("keys.velocity", { comp: this.__layer.__comp, inSpeed: sp(inEase), inInfluence: inf(inEase), outSpeed: sp(outEase), outInfluence: inf(outEase) });
  };
  P.keyTemporalContinuous = function (i) { return !!this.__key(i).continuous; };
  P.setTemporalContinuousAtKey = function (i, b) { this.__select(i); __call("keys.interpolation", { comp: this.__layer.__comp, continuous: !!b }); };
  P.keyTemporalAutoBezier = function (i) { return !!this.__key(i).autoBezier; };
  P.setTemporalAutoBezierAtKey = function (i, b) { this.__select(i); __call("keys.interpolation", { comp: this.__layer.__comp, autoBezier: !!b }); };
  P.keyRoving = function (i) { return !!this.__key(i).roving; };
  P.setRovingAtKey = function (i, b) { this.__select(i); __call("keys.interpolation", { comp: this.__layer.__comp, roving: !!b }); };
  P.keyInSpatialTangent = function (i) { return this.__key(i).spatialIn.slice(0, this.__info().dims); };
  P.keyOutSpatialTangent = function (i) { return this.__key(i).spatialOut.slice(0, this.__info().dims); };
  P.keySpatialContinuous = function (i) { return !!this.__key(i).spatialContinuous; };
  P.keySpatialAutoBezier = function (i) { return !!this.__key(i).spatialAutoBezier; };
  P.setSpatialTangentsAtKey = function (i, inT, outT) {
    var k = this.__key(i);
    __call("keys.setSpatialTangents", this.__ref({ time: k.layerTime, "in": inT, out: outT === undefined ? inT : outT }));
  };
  P.setSpatialContinuousAtKey = function (i, b) { this.__select(i); __call("keys.interpolation", { comp: this.__layer.__comp, spatial: b ? "continuousBezier" : "bezier" }); };
  P.setSpatialAutoBezierAtKey = function (i, b) { this.__select(i); __call("keys.interpolation", { comp: this.__layer.__comp, spatial: b ? "autoBezier" : "bezier" }); };
  P.keySelected = function (i) { return !!this.__key(i).selected; };
  P.setSelectedAtKey = function (i, b) {
    var k = this.__key(i);
    if (b) {
      __call("keys.select", { comp: this.__layer.__comp, add: true, keys: [{ layer: this.__layer.__id, prop: this.__uid, time: k.layerTime }] });
    } else {
      var keep = [], all = this.__keys();
      for (var j = 0; j < all.length; j++) if (all[j].selected && j !== i - 1) keep.push({ layer: this.__layer.__id, prop: this.__uid, time: all[j].layerTime });
      __call("keys.select", { comp: this.__layer.__comp, keys: keep });
    }
  };
  P.keyLabel = function () { return 0; };
  P.setLabelAtKey = function () {};
})();

// Layer / composition markers (`layer.marker`, `comp.markerProperty`): keyframes are markers.
function MarkerProperty(comp, layer) {
  this.__compId = comp;
  this.__layerRef = layer;
  this.__kind = "Property";
}
MarkerProperty.prototype = {
  constructor: MarkerProperty,
  __p: function (o) {
    o = o || {};
    o.comp = this.__compId;
    if (this.__layerRef) o.layer = this.__layerRef.__id;
    return o;
  },
  __list: function () { return __get("markers", this.__p()); },
  get name() { return "Marker"; },
  get matchName() { return "ADBE Marker"; },
  get propertyValueType() { return PropertyValueType.MARKER; },
  get propertyType() { return PropertyType.PROPERTY; },
  get numKeys() { return this.__list().length; },
  get value() { return null; },
  get canSetExpression() { return false; },
  get canVaryOverTime() { return true; },
  get isTimeVarying() { return this.numKeys > 0; },
  get parentProperty() { return this.__layerRef || null; },
  __at: function (i) {
    var l = this.__list();
    if (i < 1 || i > l.length) throw __err("marker index " + i + " is out of range 1.." + l.length);
    return l[i - 1];
  },
  keyTime: function (i) { return this.__at(i).time; },
  keyValue: function (i) {
    var m = this.__at(i);
    var v = new MarkerValue(m.comment, m.chapter, m.url, m.frameTarget, m.cuePointName);
    v.duration = m.duration;
    v.eventCuePoint = m.eventCuePoint;
    v.label = m.label;
    v.protectedRegion = m.protectedRegion;
    v.__params = m.params || [];
    return v;
  },
  nearestKeyIndex: function (t) {
    var l = this.__list(), best = 0, d = Infinity;
    for (var i = 0; i < l.length; i++) if (Math.abs(l[i].time - t) < d) { d = Math.abs(l[i].time - t); best = i + 1; }
    if (!best) throw __err("there are no markers");
    return best;
  },
  setValueAtTime: function (t, mv) {
    t = __num(t, "time");
    if (!(mv instanceof MarkerValue)) throw __err("value must be a MarkerValue");
    var p = this.__p({ time: t, duration: mv.duration || 0, comment: mv.comment, chapter: mv.chapter, url: mv.url, frameTarget: mv.frameTarget, protected: !!mv.protectedRegion, label: mv.label || 0 });
    p.cuePoint = mv.cuePointName ? { name: mv.cuePointName, navigation: !mv.eventCuePoint, params: mv.__params } : null;
    var l = this.__list(), at = -1;
    for (var i = 0; i < l.length; i++) if (Math.abs(l[i].time - t) < 1e-6) at = i;
    if (at >= 0) p.index = at;
    else p["new"] = true;
    __call("markers.set", p);
  },
  setValueAtKey: function (i, mv) { this.setValueAtTime(this.keyTime(i), mv); },
  removeKey: function (i) { this.__at(i); __call("markers.delete", this.__p({ index: i - 1 })); },
  toString: function () { return "[object Property]"; },
};

// ------------------------------------------------------------------ layers

function __layer(compId, id) {
  if (id === null || id === undefined) return null;
  var i = __get("layer", { comp: compId, layer: id });
  switch (i.kind) {
    case "text": return new TextLayer(compId, id);
    case "shape": return new ShapeLayer(compId, id);
    case "camera": return new CameraLayer(compId, id);
    case "light": return new LightLayer(compId, id);
    default: return new AVLayer(compId, id);
  }
}

function Layer() {}
Layer.prototype = {
  __init: function (comp, id, kind) {
    this.__comp = comp;
    this.__id = id;
    this.__kind = kind;
    return new Proxy(this, __attr);
  },
  __info: function () { return __get("layer", { comp: this.__comp, layer: this.__id }); },
  __p: function (o) {
    o = o || {};
    o.comp = this.__comp;
    o.layers = [this.__id];
    return o;
  },
  __p1: function (o) {
    o = o || {};
    o.comp = this.__comp;
    o.layer = this.__id;
    return o;
  },
  __switch: function (sw, v) { __call("layer.setSwitch", this.__p({ "switch": sw, value: !!v })); },
  // Comp seconds → layer seconds (keyframe times are layer time).
  __layerTime: function (t) {
    var i = this.__info();
    return (t - i.startTime) * 100 / (i.stretch || 100);
  },
  toString: function () { return "[object " + this.__kind + "]"; },
  get id() { return this.__id; },
  get name() { return this.__info().name; },
  set name(v) { __call("layer.rename", this.__p1({ name: String(v) })); },
  get index() { return this.__info().index; },
  get matchName() { return this.__info().matchName; },
  get containingComp() { return new CompItem(this.__comp); },
  get propertyDepth() { return 0; },
  get propertyType() { return PropertyType.INDEXED_GROUP; },
  get parentProperty() { return null; },
  get isModified() { return false; },
  get elided() { return false; },
  get isEffect() { return false; },
  get isMask() { return false; },
  get canSetEnabled() { return true; },
  get enabled() { return this.__info().enabled; },
  set enabled(v) { this.__switch("video", v); },
  get active() { return this.__info().active; },
  get solo() { return this.__info().solo; },
  set solo(v) { this.__switch("solo", v); },
  get locked() { return this.__info().locked; },
  set locked(v) { this.__switch("lock", v); },
  get shy() { return this.__info().shy; },
  set shy(v) { this.__switch("shy", v); },
  get audioEnabled() { return this.__info().audioEnabled; },
  set audioEnabled(v) { this.__switch("audio", v); },
  get hasVideo() { return this.__info().hasVideo; },
  get hasAudio() { return this.__info().hasAudio; },
  get audioActive() { var i = this.__info(); return i.hasAudio && i.audioEnabled; },
  get nullLayer() { return this.__info().nullLayer; },
  get isNameSet() { return true; },
  get inPoint() { return this.__info().inPoint; },
  set inPoint(v) { __call("layer.timing", this.__p({ "in": __num(v, "inPoint") })); },
  get outPoint() { return this.__info().outPoint; },
  set outPoint(v) { __call("layer.timing", this.__p({ out: __num(v, "outPoint") })); },
  get startTime() { return this.__info().startTime; },
  set startTime(v) { __call("layer.timing", this.__p({ start: __num(v, "startTime") })); },
  get stretch() { return this.__info().stretch; },
  set stretch(v) { __call("layer.timeStretch", this.__p({ percent: __num(v, "stretch") })); },
  get time() { return this.__info().time; },
  get label() { return this.__info().label; },
  set label(v) {
    var names = __get("labels");
    var n = names[__num(v, "label")];
    if (n === undefined) throw __err("label must be 0-16");
    __call("edit.label", this.__p({ label: n }));
  },
  get comment() { return this.__info().comment; },
  get selected() { return this.__info().selected; },
  set selected(v) {
    var sel = this.containingComp.__info().selectedLayers;
    var on = sel.indexOf(this.__id) >= 0;
    if (!!v !== on) __call("layer.select", { comp: this.__comp, layers: [this.__id], toggle: true });
  },
  get selectedProperties() {
    var c = this.containingComp.__info().selectedProperties, out = [];
    for (var i = 0; i < c.length; i++) if (c[i][0] === this.__id) out.push(__propOf(this, __get("node", { comp: this.__comp, layer: this.__id, uid: c[i][1] })));
    return out;
  },
  get parent() { var p = this.__info().parent; return p === null ? null : __layer(this.__comp, p); },
  set parent(v) { __call("layer.setParent", this.__p({ parent: v ? v.__id : null })); },
  setParentWithJump: function (v) { __call("layer.setParent", this.__p({ parent: v ? v.__id : null, compensate: false })); },
  get autoOrient() { return AutoOrientType[{ Off: "NO_AUTO_ORIENT", AlongPath: "ALONG_PATH", TowardsCamera: "CHARACTERS_TOWARD_CAMERA", TowardsPointOfInterest: "CAMERA_OR_POINT_OF_INTEREST" }[this.__info().autoOrient]]; },
  set autoOrient(v) {
    var m = { 4212: "off", 4213: "alongPath", 4214: "towardsPointOfInterest", 4215: "towardsCamera" }[v];
    if (!m) throw __err("Bad AutoOrientType value " + v);
    __call("layer.autoOrient", this.__p({ mode: m }));
  },
  get marker() { return new MarkerProperty(this.__comp, this); },
  get numProperties() { return __get("node", { comp: this.__comp, layer: this.__id, uid: 0 }).numProperties; },
  property: function (key) {
    if (key === undefined || key === null) throw __err("property(): missing name or index");
    if (key === "Marker" || key === "ADBE Marker" || key === "marker") return this.marker;
    var k = typeof key === "number" ? key : String(key);
    return __propOf(this, __get("child", { comp: this.__comp, layer: this.__id, uid: 0, key: k }));
  },
  propertyGroup: function () { return null; },
  canAddProperty: function () { return false; },
  addProperty: function (n) { throw __err("Can not add \"" + n + "\" to a layer: use a property group (Effects, Masks, Contents)"); },
  remove: function () { __call("edit.clear", this.__p()); },
  duplicate: function () {
    var before = this.containingComp.__info().layers;
    __call("edit.duplicate", this.__p());
    var after = this.containingComp.__info().layers;
    for (var i = 0; i < after.length; i++) if (before.indexOf(after[i]) < 0) return __layer(this.__comp, after[i]);
    return null;
  },
  copyToComp: function (comp) {
    var c = __call("edit.copy", this.__p());
    var before = comp.__info().layers;
    __call("comp.open", { comp: comp.__id });
    __call("edit.paste", { comp: comp.__id });
    return c;
  },
  moveToBeginning: function () { __call("layer.arrange", this.__p({ to: "front" })); },
  moveToEnd: function () { __call("layer.arrange", this.__p({ to: "back" })); },
  moveBefore: function (other) {
    var me = this.index, to = other.index;
    __call("layer.arrange", this.__p({ to: "index", index: me < to ? to - 1 : to }));
  },
  moveAfter: function (other) {
    var me = this.index, to = other.index;
    __call("layer.arrange", this.__p({ to: "index", index: me < to ? to : to + 1 }));
  },
  activeAtTime: function (t) { var i = this.__info(); return i.enabled && t >= i.inPoint && t < i.outPoint; },
  applyPreset: function (f) { __call("anim.applyPreset", this.__p({ path: __str(f) })); },
};

function AVLayer(comp, id) { return this.__init(comp, id, "AVLayer"); }
AVLayer.prototype = Object.create(Layer.prototype);
AVLayer.prototype.constructor = AVLayer;
(function () {
  var P = AVLayer.prototype;
  function def(name, get, set) { Object.defineProperty(P, name, { get: get, set: set }); }
  def("source", function () { return __item(this.__info().source); });
  def("width", function () { return this.__info().width; });
  def("height", function () { return this.__info().height; });
  def("threeDLayer", function () { return this.__info().threeDLayer; }, function (v) { this.__switch("threeD", v); });
  def("adjustmentLayer", function () { return this.__info().adjustmentLayer; }, function (v) { this.__switch("adjustment", v); });
  def("guideLayer", function () { return this.__info().guideLayer; }, function (v) { this.__switch("guide", v); });
  def("motionBlur", function () { return this.__info().motionBlur; }, function (v) { this.__switch("motionBlur", v); });
  def("effectsActive", function () { return this.__info().effectsActive; }, function (v) { this.__switch("fx", v); });
  def("collapseTransformation", function () { return this.__info().collapse; }, function (v) { this.__switch("collapse", v); });
  def("preserveTransparency", function () { return this.__info().preserveTransparency; }, function (v) { this.__switch("preserveTransparency", v); });
  def("quality", function () { return LayerQuality[this.__info().quality.toUpperCase()]; }, function (v) {
    var q = __enumKey(LayerQuality, v);
    if (!q) throw __err("Bad LayerQuality value " + v);
    __call("layer.quality", this.__p({ quality: q.toLowerCase() }));
  });
  def("samplingQuality", function () { return LayerSamplingQuality[this.__info().samplingQuality.toUpperCase()]; }, function (v) {
    var q = __enumKey(LayerSamplingQuality, v);
    if (!q) throw __err("Bad LayerSamplingQuality value " + v);
    __call("layer.sampling", this.__p({ sampling: q.toLowerCase() }));
  });
  def("frameBlendingType", function () { return { Off: 4012, FrameMix: 4013, PixelMotion: 4014 }[this.__info().frameBlending]; }, function (v) {
    var m = { 4012: "off", 4013: "frameMix", 4014: "pixelMotion" }[v];
    if (!m) throw __err("Bad FrameBlendingType value " + v);
    __call("layer.frameBlending", this.__p({ mode: m }));
  });
  def("frameBlending", function () { return this.__info().frameBlending !== "Off"; });
  def("blendingMode", function () { return __blendNames[this.__info().blendingMode]; }, function (v) {
    __call("layer.setBlendMode", this.__p({ mode: __blendLabel(v) }));
  });
  def("trackMatteType", function () {
    var m = this.__info().trackMatte;
    return m ? __matteEnum[m.kind] : TrackMatteType.NO_TRACK_MATTE;
  }, function (v) {
    if (v === TrackMatteType.NO_TRACK_MATTE) { __call("layer.trackMatte", this.__p({ op: "none" })); return; }
    var k = __matteKind[v];
    if (!k) throw __err("Bad TrackMatteType value " + v);
    var m = this.__info().trackMatte;
    if (m) __call("layer.setTrackMatte", this.__p1({ matte: m.layer, kind: k }));
    else __call("layer.trackMatte", this.__p({ op: k }));
  });
  def("trackMatteLayer", function () { var m = this.__info().trackMatte; return m ? __layer(this.__comp, m.layer) : null; });
  def("hasTrackMatte", function () { return !!this.__info().trackMatte; });
  def("isTrackMatte", function () { return this.__info().isTrackMatte; });
  def("timeRemapEnabled", function () { return this.__info().timeRemapEnabled; }, function (v) {
    __call("layer.enableTimeRemap", this.__p({ value: !!v }));
  });
  def("canSetTimeRemapEnabled", function () { var s = this.source; return !!s && !(s instanceof FootageItem && s.mainSource.isStill); });
  def("canSetCollapseTransformation", function () { return true; });
  def("isNameFromSource", function () { var s = this.source; return !!s && s.name === this.name; });
  def("environmentLayer", function () { return false; });
  P.setTrackMatte = function (layer, type) {
    if (!layer || type === TrackMatteType.NO_TRACK_MATTE) { this.removeTrackMatte(); return; }
    var k = __matteKind[type];
    if (!k) throw __err("Bad TrackMatteType value " + type);
    __call("layer.setTrackMatte", this.__p1({ matte: layer.__id, kind: k }));
  };
  P.removeTrackMatte = function () { __call("layer.setTrackMatte", this.__p1({ matte: null })); };
  P.sourceRectAtTime = function (t, extents) {
    return __get("sourceRect", { comp: this.__comp, layer: this.__id, time: __num(t, "time"), extents: !!extents });
  };
  P.sourcePointToComp = function (p) { return p; };
  P.compPointToSource = function (p) { return p; };
  P.replaceSource = function () { throw __err("replaceSource is not supported yet"); };
  P.openInViewer = function () { __call("layer.openLayer", this.__p1()); return null; };
})();

function TextLayer(comp, id) { return this.__init(comp, id, "TextLayer"); }
TextLayer.prototype = Object.create(AVLayer.prototype);
TextLayer.prototype.constructor = TextLayer;
Object.defineProperty(TextLayer.prototype, "source", { get: function () { return null; } });

function ShapeLayer(comp, id) { return this.__init(comp, id, "ShapeLayer"); }
ShapeLayer.prototype = Object.create(AVLayer.prototype);
ShapeLayer.prototype.constructor = ShapeLayer;
Object.defineProperty(ShapeLayer.prototype, "source", { get: function () { return null; } });

function CameraLayer(comp, id) { return this.__init(comp, id, "CameraLayer"); }
CameraLayer.prototype = Object.create(Layer.prototype);
CameraLayer.prototype.constructor = CameraLayer;

function LightLayer(comp, id) { return this.__init(comp, id, "LightLayer"); }
LightLayer.prototype = Object.create(Layer.prototype);
LightLayer.prototype.constructor = LightLayer;
Object.defineProperty(LightLayer.prototype, "lightType", {
  get: function () { return LightType[this.__info().lightType.toUpperCase()]; },
  set: function (v) {
    var k = __enumKey(LightType, v);
    if (!k) throw __err("Bad LightType value " + v);
    __call("layer.lightSettings", this.__p1({ kind: k.charAt(0) + k.substring(1).toLowerCase() }));
  },
});

// ------------------------------------------------------------------ collections

function LayerCollection(comp) {
  this.__comp = comp;
  return new Proxy(this, __coll);
}
LayerCollection.prototype = {
  constructor: LayerCollection,
  __ids: function () { return __get("item", { id: this.__comp }).layers; },
  get length() { return this.__ids().length; },
  __at: function (i) { var ids = this.__ids(); return i >= 1 && i <= ids.length ? __layer(this.__comp, ids[i - 1]) : undefined; },
  __new: function (r) { return __layer(this.__comp, r.layer); },
  __dur: function (layer, duration) {
    if (duration !== undefined && duration !== null) {
      var i = layer.__info();
      __call("layer.timing", { comp: this.__comp, layers: [layer.__id], out: i.inPoint + __num(duration, "duration") });
    }
    return layer;
  },
  add: function (item, duration) {
    if (!item || item.id === undefined) throw __err("add(): expected an item");
    var l = this.__new(__call("layer.addItem", { comp: this.__comp, item: item.id, time: 0 }));
    return this.__dur(l, duration);
  },
  addSolid: function (color, name, width, height, pixelAspect, duration) {
    var p = { comp: this.__comp, color: [color[0], color[1], color[2]] };
    if (name !== undefined) p.name = String(name);
    if (width !== undefined) p.width = Math.round(__num(width, "width"));
    if (height !== undefined) p.height = Math.round(__num(height, "height"));
    if (pixelAspect !== undefined) {
      p.pixelAspect = __num(pixelAspect, "pixelAspect");
      if (!isFinite(p.pixelAspect) || p.pixelAspect < 0.01 || p.pixelAspect > 100) {
        throw __err("pixelAspect must be between 0.01 and 100");
      }
    }
    return this.__dur(this.__new(__call("layer.newSolid", p)), duration);
  },
  addNull: function (duration) { return this.__dur(this.__new(__call("layer.newNull", { comp: this.__comp })), duration); },
  addText: function (text) {
    var t = text instanceof TextDocument ? text : new TextDocument(text === undefined ? "" : String(text));
    var p = t.__attrs();
    p.comp = this.__comp;
    if (p.text === undefined) p.text = t.text;
    return this.__new(__call("layer.newText", p));
  },
  addBoxText: function (size, text) {
    var c = __get("item", { id: this.__comp });
    var w = size[0], h = size[1];
    var t = text instanceof TextDocument ? text : new TextDocument(text === undefined ? "" : String(text));
    var p = t.__attrs();
    p.comp = this.__comp;
    if (p.text === undefined) p.text = t.text;
    p.box = [(c.width - w) / 2, (c.height - h) / 2, w, h];
    return this.__new(__call("layer.newText", p));
  },
  addShape: function () { return this.__new(__call("layer.newShape", { comp: this.__comp, kind: "none" })); },
  addCamera: function (name, center) {
    var p = { comp: this.__comp, name: String(name) };
    if (center) p.poi = [center[0], center[1], 0];
    return this.__new(__call("layer.newCamera", p));
  },
  addLight: function (name, center) {
    var p = { comp: this.__comp, name: String(name), kind: "Point" };
    if (center) p.poi = [center[0], center[1], 0];
    return this.__new(__call("layer.newLight", p));
  },
  precompose: function (indices, name, moveAllAttributes) {
    var ids = this.__ids(), sel = [];
    for (var i = 0; i < indices.length; i++) sel.push(ids[indices[i] - 1]);
    var r = __call("layer.precompose", { comp: this.__comp, layers: sel, name: String(name), mode: moveAllAttributes === false ? "leave" : "move" });
    return new CompItem(r.comp);
  },
  byName: function (name) {
    var ids = this.__ids();
    for (var i = 0; i < ids.length; i++) {
      var l = __get("layer", { comp: this.__comp, layer: ids[i] });
      if (l.name === name) return __layer(this.__comp, ids[i]);
    }
    return null;
  },
  toString: function () { return "[object LayerCollection]"; },
};

function ItemCollection(folder) {
  this.__folder = folder;
  return new Proxy(this, __coll);
}
ItemCollection.prototype = {
  constructor: ItemCollection,
  __ids: function () {
    if (this.__folder === 0) return __get("project").items;
    return __get("item", { id: this.__folder }).children;
  },
  get length() { return this.__ids().length; },
  __at: function (i) { var ids = this.__ids(); return i >= 1 && i <= ids.length ? __item(ids[i - 1]) : undefined; },
  __place: function (id) {
    if (this.__folder) __call("project.move", { items: [id], folder: this.__folder });
    return __item(id);
  },
  addComp: function (name, width, height, pixelAspect, duration, frameRate) {
    var r = __call("comp.new", {
      name: String(name), width: Math.round(__num(width, "width")), height: Math.round(__num(height, "height")),
      pixelAspect: __num(pixelAspect, "pixelAspect"), duration: __num(duration, "duration"), frameRate: __num(frameRate, "frameRate"), open: false,
    });
    return this.__place(r.comp);
  },
  addFolder: function (name) {
    var r = __call("project.newFolder", { name: String(name) });
    var id = r && typeof r === "object" ? (r.folder || r.item || r.id) : r;
    return this.__place(id);
  },
  toString: function () { return "[object ItemCollection]"; },
};

// ------------------------------------------------------------------ items

function __item(id) {
  if (id === null || id === undefined) return null;
  if (id === 0) return new FolderItem(0);
  var i = __get("item", { id: id });
  switch (i.kind) {
    case "comp": return new CompItem(id);
    case "folder": return new FolderItem(id);
    default: return new FootageItem(id);
  }
}

function Item() {}
Item.prototype = {
  __info: function () { return __get("item", { id: this.__id }); },
  toString: function () { return "[object " + this.__kind + "]"; },
  get id() { return this.__id; },
  get name() { return this.__info().name; },
  set name(v) { __call("project.rename", { item: this.__id, name: String(v) }); },
  get typeName() { return this.__info().typeName; },
  get comment() { return this.__info().comment; },
  set comment(v) { __call("project.setComment", { items: [this.__id], comment: String(v) }); },
  get label() { return this.__info().label; },
  set label(v) { __call("project.setLabel", { items: [this.__id], label: __num(v, "label") }); },
  get parentFolder() { return __item(this.__info().parent); },
  set parentFolder(f) { __call("project.move", { items: [this.__id], folder: f && f.__id ? f.__id : null }); },
  get selected() { return this.__info().selected; },
  set selected(v) { if (v) __call("project.select", { items: [this.__id], add: true }); },
  get usedIn() { return this.__info().usedIn.map(function (c) { return new CompItem(c); }); },
  remove: function () { __call("project.delete", { items: [this.__id] }); },
};

function AVItem() {}
AVItem.prototype = Object.create(Item.prototype);
(function () {
  function def(name, get, set) { Object.defineProperty(AVItem.prototype, name, { get: get, set: set }); }
  def("width", function () { return this.__info().width; }, function (v) { this.__settings({ width: Math.round(__num(v, "width")) }); });
  def("height", function () { return this.__info().height; }, function (v) { this.__settings({ height: Math.round(__num(v, "height")) }); });
  def("pixelAspect", function () { return this.__info().pixelAspect; }, function (v) { this.__settings({ pixelAspect: __num(v) }); });
  def("duration", function () { return this.__info().duration; }, function (v) { this.__settings({ duration: __num(v, "duration") }); });
  def("frameRate", function () { return this.__info().frameRate; }, function (v) { this.__settings({ frameRate: __num(v, "frameRate") }); });
  def("frameDuration", function () { var f = this.__info().frameRate; return f ? 1 / f : 0; }, function (v) { this.__settings({ frameRate: 1 / __num(v) }); });
  def("hasVideo", function () { return this.__info().hasVideo; });
  def("hasAudio", function () { return this.__info().hasAudio; });
  def("footageMissing", function () { return !!this.__info().footageMissing; });
  def("useProxy", function () { return false; });
  def("proxySource", function () { return null; });
  def("isMediaReplacementCompatible", function () { return false; });
  def("time", function () { return 0; });
})();
AVItem.prototype.__settings = function () { throw __err("can't change this item's settings from a script"); };

function CompItem(id) {
  this.__id = id;
  this.__kind = "CompItem";
}
CompItem.prototype = Object.create(AVItem.prototype);
CompItem.prototype.constructor = CompItem;
(function () {
  var P = CompItem.prototype;
  function def(name, get, set) { Object.defineProperty(P, name, { get: get, set: set }); }
  P.__settings = function (o) { o.comp = this.__id; __call("comp.settings", o); };
  def("typeName", function () { return "Composition"; });
  def("layers", function () { return new LayerCollection(this.__id); });
  def("numLayers", function () { return this.__info().layers.length; });
  def("selectedLayers", function () { var c = this.__id; return this.__info().selectedLayers.map(function (l) { return __layer(c, l); }); });
  def("selectedProperties", function () {
    var c = this.__id, out = [];
    this.__info().selectedProperties.forEach(function (lp) {
      var L = __layer(c, lp[0]);
      out.push(__propOf(L, __get("node", { comp: c, layer: lp[0], uid: lp[1] })));
    });
    return out;
  });
  def("activeCamera", function () {
    var ids = this.__info().layers;
    for (var i = 0; i < ids.length; i++) {
      var l = __get("layer", { comp: this.__id, layer: ids[i] });
      if (l.kind === "camera" && l.enabled && l.active) return __layer(this.__id, ids[i]);
    }
    return null;
  });
  def("time", function () { return this.__info().time; }, function (v) { __call("time.set", { comp: this.__id, time: __num(v, "time") }); });
  def("workAreaStart", function () { return this.__info().workAreaStart; }, function (v) {
    var i = this.__info();
    __call("comp.workArea", { comp: this.__id, start: __num(v), end: __num(v) + i.workAreaDuration });
  });
  def("workAreaDuration", function () { return this.__info().workAreaDuration; }, function (v) {
    var i = this.__info();
    __call("comp.workArea", { comp: this.__id, start: i.workAreaStart, end: i.workAreaStart + __num(v) });
  });
  def("bgColor", function () { return this.__info().bgColor; }, function (v) { this.__settings({ background: [v[0], v[1], v[2]] }); });
  def("displayStartTime", function () { return this.__info().displayStartTime; }, function (v) { this.__settings({ startTime: __num(v) }); });
  def("hideShyLayers", function () { return this.__info().hideShyLayers; }, function (v) { __call("comp.setSwitch", { comp: this.__id, "switch": "hideShy", value: !!v }); });
  def("motionBlur", function () { return this.__info().motionBlur; }, function (v) { __call("comp.setSwitch", { comp: this.__id, "switch": "motionBlur", value: !!v }); });
  def("frameBlending", function () { return this.__info().frameBlending; }, function (v) { __call("comp.setSwitch", { comp: this.__id, "switch": "frameBlending", value: !!v }); });
  def("draft3d", function () { return this.__info().draft3d; }, function (v) { __call("comp.setSwitch", { comp: this.__id, "switch": "draft3d", value: !!v }); });
  def("shutterAngle", function () { return this.__info().shutterAngle; }, function (v) { this.__settings({ shutterAngle: __num(v) }); });
  def("shutterPhase", function () { return this.__info().shutterPhase; }, function (v) { this.__settings({ shutterPhase: __num(v) }); });
  def("motionBlurSamplesPerFrame", function () { return this.__info().motionBlurSamplesPerFrame; }, function (v) { this.__settings({ motionBlurSamples: __num(v) }); });
  def("preserveNestedFrameRate", function () { return this.__info().preserveNestedFrameRate; }, function (v) { this.__settings({ preserveFrameRate: !!v }); });
  def("preserveNestedResolution", function () { return this.__info().preserveNestedResolution; }, function (v) { this.__settings({ preserveResolution: !!v }); });
  def("renderer", function () { return this.__info().renderer; }, function (v) {
    this.__settings({ renderer: String(v).toLowerCase().indexOf("advanced") >= 0 ? "advanced3D" : "classic3D" });
  });
  def("renderers", function () { return ["ADBE Classic 3D", "ADBE Advanced 3d"]; });
  def("markerProperty", function () { return new MarkerProperty(this.__id, null); });
  def("hasVideo", function () { return true; });
  P.layer = function (a, b) {
    var ids = this.__info().layers;
    if (typeof a === "number") {
      if (b !== undefined) {
        // layer(otherLayer, relIndex)
        throw __err("layer(index, relIndex) is not supported");
      }
      if (a < 1 || a > ids.length || Math.floor(a) !== a) throw __err("layer index " + a + " is out of range 1.." + ids.length);
      return __layer(this.__id, ids[a - 1]);
    }
    if (a && a.__id !== undefined && typeof b === "number") {
      var j = a.index + b;
      return j >= 1 && j <= ids.length ? __layer(this.__id, ids[j - 1]) : null;
    }
    return new LayerCollection(this.__id).byName(String(a));
  };
  P.openInViewer = function () { __call("comp.open", { comp: this.__id }); return null; };
  P.duplicate = function () { return __item(__call("project.duplicate", { items: [this.__id] }).items[0]); };
  // Essential Graphics (templates are EffectCraft's open .ectemplate files).
  P.__eg = function () { return __call("essential.list", { comp: this.__id }); };
  P.__controllers = function () {
    var out = [];
    (function walk(list) {
      for (var i = 0; i < list.length; i++) {
        if (list[i].kind === "property" || list[i].kind === "mirror" || list[i].kind === "media") out.push(list[i]);
        if (list[i].children) walk(list[i].children);
      }
    })(this.__eg().controls);
    return out;
  };
  def("motionGraphicsTemplateName", function () { return this.__eg().name; }, function (v) {
    __call("essential.setName", { comp: this.__id, name: String(v) });
  });
  def("motionGraphicsTemplateControllerCount", function () { return this.__controllers().length; });
  P.getMotionGraphicsTemplateControllerName = function (i) {
    var c = this.__controllers();
    i = __num(i, "index");
    if (i < 1 || i > c.length) throw __err("controller index " + i + " is out of range 1.." + c.length);
    return c[i - 1].name;
  };
  P.setMotionGraphicsControllerName = function (i, name) {
    var c = this.__controllers();
    i = __num(i, "index");
    if (i < 1 || i > c.length) throw __err("controller index " + i + " is out of range 1.." + c.length);
    __call("essential.rename", { comp: this.__id, control: c[i - 1].id, name: String(name) });
    return String(name);
  };
  P.openInEssentialGraphics = function () { __call("comp.openInEssentialGraphics", { comp: this.__id }); };
  P.exportAsMotionGraphicsTemplate = function (overwrite, path) {
    var f = __str(path);
    if (!f) return false;
    if (!/\.[A-Za-z0-9]+$/.test(f)) f += ".ectemplate";
    if (!overwrite && JSON.parse(__file("exists", f)) === true) return false;
    if (this.__controllers().length === 0) return false;
    // The file gate applies to templates too.
    __file("write", f, "");
    __call("essential.exportTemplate", { comp: this.__id, path: f });
    return true;
  };
})();

function FootageItem(id) {
  this.__id = id;
  this.__kind = "FootageItem";
}
FootageItem.prototype = Object.create(AVItem.prototype);
FootageItem.prototype.constructor = FootageItem;
Object.defineProperty(FootageItem.prototype, "file", {
  get: function () { var f = this.__info().file; return f ? new File(f) : null; },
});
Object.defineProperty(FootageItem.prototype, "mainSource", {
  get: function () {
    var i = this.__info();
    if (i.kind === "solid") return { isStill: true, color: i.color, hasAlpha: false, toString: function () { return "[object SolidSource]"; } };
    return { isStill: !!i.isStill, file: i.file ? new File(i.file) : null, missingFootagePath: i.footageMissing ? i.file : "", loop: i.loop, hasAlpha: false, toString: function () { return "[object FileSource]"; } };
  },
});
FootageItem.prototype.replace = function (f) { __call("file.replaceFootage", { item: this.__id, path: __str(f) }); };
FootageItem.prototype.replaceWithSolid = function (color, name, w, h) {
  __call("file.replaceWithSolid", { item: this.__id, color: [color[0], color[1], color[2]], width: w, height: h });
  if (name !== undefined) this.name = name;
};
FootageItem.prototype.openInViewer = function () { return null; };

function FolderItem(id) {
  this.__id = id;
  this.__kind = "FolderItem";
}
FolderItem.prototype = Object.create(Item.prototype);
FolderItem.prototype.constructor = FolderItem;
(function () {
  var P = FolderItem.prototype;
  P.__info = function () {
    if (this.__id === 0) return { id: 0, name: "Root", typeName: "Folder", comment: "", label: 0, parent: null, selected: false, usedIn: [], children: [] };
    return __get("item", { id: this.__id });
  };
  Object.defineProperty(P, "items", { get: function () { return new ItemCollection(this.__id); } });
  Object.defineProperty(P, "numItems", { get: function () { return new ItemCollection(this.__id).length; } });
  Object.defineProperty(P, "parentFolder", {
    get: function () { return this.__id === 0 ? null : __item(this.__info().parent); },
    set: function (f) { __call("project.move", { items: [this.__id], folder: f && f.__id ? f.__id : null }); },
  });
  P.item = function (i) {
    var v = new ItemCollection(this.__id).__at(__num(i, "index"));
    if (v === undefined) throw __err("item index " + i + " is out of range");
    return v;
  };
})();

// ------------------------------------------------------------------ render queue

function OutputModule(rq, n) {
  this.__rq = rq;
  this.__n = n;
}
OutputModule.prototype = {
  constructor: OutputModule,
  __info: function () { return this.__rq.__info(); },
  get name() { var i = this.__info(); return this.__n === 1 ? i.outputModuleSummary : i.extraOutputs[this.__n - 2].summary; },
  get file() {
    var i = this.__info();
    var p = this.__n === 1 ? i.outputPath : i.extraOutputs[this.__n - 2].outputPath;
    return p ? new File(p) : null;
  },
  set file(f) { __call("renderQueue.setOutput", { item: this.__rq.__id, module: this.__n, path: __str(f) }); },
  get templates() { return ["H.264", "ProRes", "PNG Sequence", "JPEG Sequence", "TIFF Sequence", "OpenEXR Sequence", "GIF"]; },
  applyTemplate: function (name) {
    var n = String(name).toLowerCase();
    var f = n.indexOf("prores") >= 0 ? "prores" : n.indexOf("png") >= 0 || n.indexOf("lossless") >= 0 ? "png" : n.indexOf("jpeg") >= 0 || n.indexOf("jpg") >= 0 ? "jpeg"
      : n.indexOf("tiff") >= 0 ? "tiff" : n.indexOf("exr") >= 0 ? "exr" : n.indexOf("gif") >= 0 ? "gif" : n.indexOf("264") >= 0 ? "h264" : null;
    if (!f) throw __err("Unknown output module template \"" + name + "\"");
    __call("renderQueue.setOutputModule", { item: this.__rq.__id, module: this.__n, format: f });
  },
  remove: function () { throw __err("removing output modules is not supported"); },
  toString: function () { return "[object OutputModule]"; },
};

var __rqStatus = { "Unqueued": 3014, "Queued": 3015, "Needs Output": 3013, "Rendering": 3016, "Done": 3019, "User Stopped": 3017, "Failed": 3018 };

function RenderQueueItem(id) { this.__id = id; }
RenderQueueItem.prototype = {
  constructor: RenderQueueItem,
  __info: function () {
    var l = __call("renderQueue.list", {}).items;
    for (var i = 0; i < l.length; i++) if (l[i].id === this.__id) return l[i];
    throw __err("the render queue item no longer exists");
  },
  get comp() { return new CompItem(this.__info().comp); },
  get status() {
    var s = this.__info().statusLabel;
    for (var k in __rqStatus) if (s.indexOf(k) === 0) return __rqStatus[k];
    return RQItemStatus.QUEUED;
  },
  get render() { return this.__info().render; },
  set render(v) { __call("renderQueue.setRender", { item: this.__id, render: !!v }); },
  get numOutputModules() { return this.__info().outputModules; },
  get outputModules() {
    var n = this.numOutputModules, out = [];
    for (var i = 1; i <= n; i++) out.push(new OutputModule(this, i));
    return out;
  },
  outputModule: function (i) {
    if (i < 1 || i > this.numOutputModules) throw __err("output module index " + i + " is out of range");
    return new OutputModule(this, i);
  },
  get timeSpanStart() { var s = this.__info().settings; return s.start !== undefined ? s.start : 0; },
  set timeSpanStart(v) { __call("renderQueue.setRenderSettings", { item: this.__id, timeSpan: "custom", start: __num(v), end: __num(v) + this.timeSpanDuration }); },
  get timeSpanDuration() { var s = this.__info().settings; return s.end !== undefined && s.start !== undefined ? s.end - s.start : this.comp.duration; },
  set timeSpanDuration(v) { var s = this.timeSpanStart; __call("renderQueue.setRenderSettings", { item: this.__id, timeSpan: "custom", start: s, end: s + __num(v) }); },
  get elapsedSeconds() { return this.__info().render_time || 0; },
  get startTime() { var s = this.__info().started; return s ? new Date(s * 1000) : null; },
  get logType() { return LogType.ERRORS_ONLY; },
  set logType(v) {},
  get skipFrames() { return 0; },
  outputModuleAdd: function () { __call("render.addOutputModule", { item: this.__id }); },
  remove: function () { __call("renderQueue.remove", { item: this.__id }); },
  duplicate: function () {
    var before = __call("renderQueue.list", {}).items.map(function (i) { return i.id; });
    __call("renderQueue.duplicate", { item: this.__id });
    var after = __call("renderQueue.list", {}).items;
    for (var i = 0; i < after.length; i++) if (before.indexOf(after[i].id) < 0) return new RenderQueueItem(after[i].id);
    return null;
  },
  applyTemplate: function () {},
  getSettings: function () { return this.__info().settings; },
  setSettings: function (o) { __call("renderQueue.setRenderSettings", Object.assign({ item: this.__id }, o)); },
  toString: function () { return "[object RenderQueueItem]"; },
};

function RQItemCollection() { return new Proxy(this, __coll); }
RQItemCollection.prototype = {
  constructor: RQItemCollection,
  __list: function () { return __call("renderQueue.list", {}).items; },
  get length() { return this.__list().length; },
  __at: function (i) { var l = this.__list(); return i >= 1 && i <= l.length ? new RenderQueueItem(l[i - 1].id) : undefined; },
  add: function (comp) {
    if (!(comp instanceof CompItem)) throw __err("add(): expected a CompItem");
    __call("renderQueue.add", { comp: comp.__id });
    var l = this.__list();
    return new RenderQueueItem(l[l.length - 1].id);
  },
  toString: function () { return "[object RQItemCollection]"; },
};

function RenderQueue() {}
RenderQueue.prototype = {
  constructor: RenderQueue,
  get items() { return new RQItemCollection(); },
  get numItems() { return new RQItemCollection().length; },
  get rendering() { return !!__call("renderQueue.list", {}).rendering; },
  get canQueueInAME() { return false; },
  get queueNotify() { return false; },
  set queueNotify(v) {},
  item: function (i) {
    var v = new RQItemCollection().__at(__num(i, "index"));
    if (v === undefined) throw __err("render queue index " + i + " is out of range");
    return v;
  },
  render: function () { __call("renderQueue.render", { wait: true }); },
  stopRendering: function () { __call("renderQueue.stop", {}); },
  pauseRendering: function () {},
  showWindow: function () {},
  queueInAME: function () { throw __err("Adobe Media Encoder is not available: render with app.project.renderQueue.render()"); },
  toString: function () { return "[object RenderQueue]"; },
};

// ------------------------------------------------------------------ project and app

function ImportOptions(file) {
  if (!(this instanceof ImportOptions)) return new ImportOptions(file);
  this.file = file || null;
  this.importAs = ImportAsType.FOOTAGE;
  this.sequence = false;
  this.forceAlphabetical = false;
  this.rangeStart = 0;
  this.rangeEnd = 0;
}
ImportOptions.prototype.canImportAs = function (t) { return t === ImportAsType.FOOTAGE; };

function Project() {}
Project.prototype = {
  constructor: Project,
  __info: function () { return __get("project"); },
  get items() { return new ItemCollection(0); },
  get numItems() { return this.__info().items.length; },
  get rootFolder() { return new FolderItem(0); },
  get activeItem() { return __item(this.__info().active); },
  get selection() { return this.__info().selection.map(__item); },
  get file() { var p = this.__info().path; return p ? new File(p) : null; },
  get dirty() { return this.__info().dirty; },
  get renderQueue() { return new RenderQueue(); },
  get bitsPerChannel() { return this.__info().bitsPerChannel; },
  set bitsPerChannel(v) { __call("file.projectSettings", { bitDepth: __num(v) }); },
  get linearBlending() { return this.__info().linearBlending; },
  set linearBlending(v) { __call("file.projectSettings", { blendLinear: !!v }); },
  get linearizeWorkingSpace() { return this.__info().linearizeWorkingSpace; },
  set linearizeWorkingSpace(v) { __call("file.projectSettings", { linearize: !!v }); },
  get workingSpace() { return this.__info().workingSpace; },
  set workingSpace(v) { __call("file.projectSettings", { workingSpace: String(v) || "none" }); },
  get timeDisplayType() { return this.__info().timeDisplayFrames ? TimeDisplayType.FRAMES : TimeDisplayType.TIMECODE; },
  set timeDisplayType(v) { __call("file.projectSettings", { timeDisplay: v === TimeDisplayType.FRAMES ? "frames" : "timecode" }); },
  get expressionEngine() { return "javascript-1.0"; },
  set expressionEngine(v) {},
  item: function (i) {
    var v = new ItemCollection(0).__at(__num(i, "index"));
    if (v === undefined) throw __err("item index " + i + " is out of range 1.." + this.numItems);
    return v;
  },
  itemByID: function (id) {
    try { return __item(__num(id)); } catch (e) { return null; }
  },
  layerByID: function () { return null; },
  save: function (file) {
    if (file) __call("file.saveAs", { path: __str(file) });
    else __call("file.save", {});
    return true;
  },
  saveAs: function (file) { return this.save(file); },
  saveWithDialog: function () { if (this.file) return this.save(); return false; },
  close: function (opt) {
    if (opt === CloseOptions.SAVE_CHANGES && this.file) this.save();
    __call("file.newProject", {});
    return true;
  },
  importFile: function (opts) {
    var f = opts instanceof ImportOptions ? opts.file : opts;
    if (!f) throw __err("importFile(): no file");
    var r = __call("file.import", { paths: [__str(f)] });
    if (r.errors && r.errors.length) throw __err(r.errors.join("; "));
    return __item(r.items[0]);
  },
  importFileWithDialog: function () { return null; },
  importPlaceholder: function (name, w, h, rate, dur) {
    var r = __call("file.importPlaceholder", { name: String(name), width: w, height: h, frameRate: rate, duration: dur });
    var id = r && typeof r === "object" ? (r.item || r.id) : r;
    return __item(id);
  },
  consolidateFootage: function () { return __call("file.consolidateFootage", {}); },
  removeUnusedFootage: function () { return __call("file.removeUnusedFootage", {}); },
  reduceProject: function (items) {
    __call("project.select", { items: items.map(function (i) { return i.__id; }) });
    return __call("file.reduceProject", {});
  },
  autoFixExpressions: function () {},
  showWindow: function () {},
  toString: function () { return "[object Project]"; },
};

var __tasks = {};
var app = {
  get project() { return new Project(); },
  get version() { return __get("app").version; },
  get buildName() { return __get("app").buildName; },
  get buildNumber() { return 0; },
  get isWatchFolder() { return false; },
  get isRenderEngine() { return false; },
  get isoLanguage() { return "en_US"; },
  get language() { return 0; },
  get memoryInUse() { return 0; },
  get effects() { return __get("effects").map(function (e) { return { displayName: e.name, matchName: e.matchName, category: e.category, version: "1.0" }; }); },
  get activeViewer() { return null; },
  get availableGPUAccelTypes() { return []; },
  exitAfterLaunchAndEval: false,
  exitCode: 0,
  saveProjectOnCrash: true,
  onError: null,
  settings: {
    __s: {},
    haveSetting: function (sec, key) { return (sec + "/" + key) in this.__s; },
    getSetting: function (sec, key) { return this.__s[sec + "/" + key] || ""; },
    saveSetting: function (sec, key, v) { this.__s[sec + "/" + key] = String(v); },
  },
  preferences: {
    havePref: function () { return false; },
    getPrefAsString: function () { return ""; },
    getPrefAsLong: function () { return 0; },
    getPrefAsBool: function () { return false; },
    getPrefAsFloat: function () { return 0; },
    savePrefAsString: function () {},
    savePrefAsLong: function () {},
    savePrefAsBool: function () {},
    savePrefAsFloat: function () {},
    reload: function () {},
  },
  newProject: function () { __call("file.newProject", {}); return new Project(); },
  open: function (file) {
    if (!file) return null;
    __call("file.open", { path: __str(file) });
    return new Project();
  },
  quit: function () {},
  beginUndoGroup: function (name) { __undo("begin", name === undefined ? "Script" : String(name)); },
  endUndoGroup: function () { __undo("end", ""); },
  beginSuppressDialogs: function () {},
  endSuppressDialogs: function () {},
  purge: function () { __call("edit.purge", { what: "all" }); },
  findMenuCommandId: function (name) { return __get("menuCommandId", { name: String(name) }); },
  executeCommand: function (id) {
    var cmd = typeof id === "number" ? __get("commandAt", { index: id }) : String(id);
    if (!cmd) throw __err("Unknown command id " + id);
    return __call(cmd, {});
  },
  scheduleTask: function (code, delay, repeat) { return __task("schedule", String(code), !!repeat); },
  cancelTask: function (id) { __task("cancel", id); },
  setMemoryUsageLimits: function () {},
  setMultiFrameRenderingConfig: function () {},
  setSavePreferencesOnQuit: function () {},
  activate: function () {},
  watchFolder: function () { throw __err("watch folders are not supported"); },
  pauseWatchFolder: function () {},
  endWatchFolder: function () {},
  parseSwatchFile: function () { throw __err("parseSwatchFile is not supported"); },
  // EffectCraft: run any engine command by id with parameters (see `command.list`).
  run: function (id, params) { return __call(String(id), params || {}); },
  toString: function () { return "[object Application]"; },
};

// The completion value of a script, as JSON text for the host.
function __result(v) {
  if (v === undefined) return undefined;
  if (v === null || typeof v === "number" || typeof v === "boolean" || typeof v === "string") return JSON.stringify(v);
  if (v instanceof Layer || v instanceof Item || v instanceof PropertyBase || v instanceof Project || v instanceof TextDocument) {
    var d = { type: v.toString() };
    try { d.name = v.name; } catch (e) {}
    if (v instanceof TextDocument) d.text = v.text;
    return JSON.stringify(d);
  }
  try {
    var s = JSON.stringify(v);
    return s === undefined ? JSON.stringify(String(v)) : s;
  } catch (e) {
    return JSON.stringify(String(v));
  }
}
