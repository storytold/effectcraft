// EffectCraft scripting: ScriptUI, the dialog / panel object model After Effects scripts use
// (`new Window("dialog", "Title")`, `win.add("button", undefined, "OK")`, `onClick`, `show()`…).
//
// Written from the behaviour Adobe's public JavaScript Tools Guide documents. Controls are plain
// JavaScript objects; after each script step the host reads `__uiSnapshot()` (a JSON
// description of the open windows) and lays them out, frontends draw them, and user actions come
// back through `__uiDispatch(window, control, kind, valueJson)`. Modal dialogs block in
// `show()` (`__uiModal`) until they close, as in After Effects.

var __uiWins = [];
// show() results of closed dialogs, by window id.
var __uiResults = {};

function __uiModalResult(id) {
  var r = __uiResults[id];
  if (r !== undefined) {
    delete __uiResults[id];
    return r;
  }
  return __uiWin(id) ? null : 2;
}

function __dim(w, h) {
  var d = [w, h];
  Object.defineProperty(d, "width", { get: function () { return this[0]; }, set: function (v) { this[0] = v; } });
  Object.defineProperty(d, "height", { get: function () { return this[1]; }, set: function (v) { this[1] = v; } });
  return d;
}
function __bounds4(l, t, r, b) {
  var a = [l, t, r, b];
  Object.defineProperty(a, "left", { get: function () { return this[0]; } });
  Object.defineProperty(a, "top", { get: function () { return this[1]; } });
  Object.defineProperty(a, "right", { get: function () { return this[2]; } });
  Object.defineProperty(a, "bottom", { get: function () { return this[3]; } });
  Object.defineProperty(a, "x", { get: function () { return this[0]; } });
  Object.defineProperty(a, "y", { get: function () { return this[1]; } });
  Object.defineProperty(a, "width", { get: function () { return this[2] - this[0]; } });
  Object.defineProperty(a, "height", { get: function () { return this[3] - this[1]; } });
  return a;
}
function __toBounds(b) {
  if (b === undefined || b === null) return null;
  if (b.length === 4) return [__num(b[0]), __num(b[1]), __num(b[2]), __num(b[3])];
  if (b.width !== undefined) {
    var x = b.x || b.left || 0, y = b.y || b.top || 0;
    return [x, y, x + __num(b.width), y + __num(b.height)];
  }
  return null;
}

var __uiTypes = {
  window: "Window", panel: "Panel", group: "Group", button: "Button", iconbutton: "IconButton",
  statictext: "StaticText", edittext: "EditText", checkbox: "Checkbox", radiobutton: "RadioButton",
  slider: "Slider", scrollbar: "Scrollbar", progressbar: "Progressbar", dropdownlist: "DropDownList",
  listbox: "ListBox", tabbedpanel: "TabbedPanel", tab: "Tab", image: "ScriptUIImage"
};
var __uiContainers = { window: 1, panel: 1, group: 1, tabbedpanel: 1, tab: 1 };

function Window(type, title, bounds, props) {
  if (!(this instanceof Window)) return new Window(type, title, bounds, props);
  type = String(type === undefined ? "window" : type);
  var res = null;
  if (type.indexOf("{") >= 0) {
    // A resource string: `dialog { text: 'Title', ok: Button { text: 'OK' } }`.
    res = __uiParseRes(type);
    type = res.type;
    var p0 = __uiResProps(res).props;
    if (title === undefined && p0.text !== undefined) title = p0.text;
    if (bounds === undefined && p0.bounds !== undefined) bounds = p0.bounds;
    if (props === undefined && p0.properties !== undefined) props = p0.properties;
  }
  type = type.toLowerCase();
  if (type !== "dialog" && type !== "palette" && type !== "window") throw __err("Bad window type " + type);
  __uiInit(this, "window", null, null, bounds, title, props);
  this.__kind = type;
  this.__root = true;
  __uiWins.push(this);
  if (res) __uiApplyRes(this, res);
}

// ---------------------------------------------------------------- resource strings
//
// The resource specification the JavaScript Tools Guide documents: `type { name: value, … }`
// where a value is a string ('…' or "…"), a number, true / false, an array `[…]`, an object
// literal `{…}` (e.g. `properties: {multiline: true}`) or a nested element `Type { … }` (a
// child control, also reachable as `parent.name`). Comments are allowed.

function __uiParseRes(src) {
  var s = String(src), i = 0;
  function fail(m) { throw __err("ScriptUI resource string: " + m + " at offset " + i); }
  function ws() {
    for (;;) {
      while (i < s.length && /\s/.test(s.charAt(i))) i++;
      if (s.substring(i, i + 2) === "//") {
        while (i < s.length && s.charAt(i) !== "\n") i++;
        continue;
      }
      if (s.substring(i, i + 2) === "/*") {
        var e = s.indexOf("*/", i + 2);
        i = e < 0 ? s.length : e + 2;
        continue;
      }
      return;
    }
  }
  function ident() {
    ws();
    var m = /^[A-Za-z_$][\w$]*/.exec(s.slice(i));
    if (!m) fail("expected a name");
    i += m[0].length;
    return m[0];
  }
  function str(q) {
    i++;
    var out = "";
    while (i < s.length && s.charAt(i) !== q) {
      var c = s.charAt(i);
      if (c === "\\") {
        i++;
        var e = s.charAt(i);
        out += e === "n" ? "\n" : e === "t" ? "\t" : e === "r" ? "\r" : e;
      } else out += c;
      i++;
    }
    if (i >= s.length) fail("unterminated string");
    i++;
    return out;
  }
  function value() {
    ws();
    var c = s.charAt(i);
    if (c === "'" || c === '"') return str(c);
    if (c === "[") {
      i++;
      var a = [];
      ws();
      if (s.charAt(i) === "]") {
        i++;
        return a;
      }
      for (;;) {
        a.push(value());
        ws();
        if (s.charAt(i) === ",") {
          i++;
          // A trailing comma, as JavaScript allows.
          ws();
          if (s.charAt(i) === "]") {
            i++;
            return a;
          }
          continue;
        }
        if (s.charAt(i) === "]") {
          i++;
          return a;
        }
        fail("expected , or ]");
      }
    }
    if (c === "{") return { __object: body() };
    var m = /^[-+]?(\d+\.?\d*|\.\d+)([eE][-+]?\d+)?/.exec(s.slice(i));
    if (m) {
      i += m[0].length;
      return Number(m[0]);
    }
    var id = ident();
    if (id === "true") return true;
    if (id === "false") return false;
    if (id === "null" || id === "undefined") return null;
    ws();
    if (s.charAt(i) === "{") return { __element: id, list: body() };
    // A bare word (an enum-like value).
    return id;
  }
  function body() {
    i++;
    var list = [];
    for (;;) {
      ws();
      if (s.charAt(i) === "}") {
        i++;
        return list;
      }
      var k = s.charAt(i) === "'" || s.charAt(i) === '"' ? str(s.charAt(i)) : ident();
      ws();
      if (s.charAt(i) !== ":") fail("expected : after " + k);
      i++;
      list.push([k, value()]);
      ws();
      if (s.charAt(i) === ",") {
        i++;
        continue;
      }
      if (s.charAt(i) === "}") {
        i++;
        return list;
      }
      fail("expected , or }");
    }
  }
  var t = ident();
  ws();
  if (s.charAt(i) !== "{") fail("expected { after " + t);
  var list = body();
  ws();
  if (i < s.length) fail("unexpected text after the resource");
  return { type: t, list: list };
}

// The plain JavaScript value of a parsed resource value (object literals become objects).
function __uiResPlain(v) {
  if (v instanceof Array) return v.map(__uiResPlain);
  if (v && typeof v === "object" && v.__object) {
    var o = {};
    for (const elem of v.__object) o[elem[0]] = __uiResPlain(elem[1]);
    return o;
  }
  return v;
}

// A resource's own properties and its child elements (in source order).
function __uiResProps(res) {
  var props = {}, kids = [];
  for (const elem2 of res.list) {
    var e = elem2;
    if (e[1] && typeof e[1] === "object" && e[1].__element) kids.push([e[0], { type: e[1].__element, list: e[1].list }]);
    else props[e[0]] = __uiResPlain(e[1]);
  }
  return { props: props, kids: kids };
}

var __uiResSkip = { text: 1, bounds: 1, properties: 1, minvalue: 1, maxvalue: 1, items: 1 };
var __uiSizes = { preferredSize: 1, minimumSize: 1, maximumSize: 1, size: 1, location: 1 };

// Set a control's properties and create its children from a parsed resource.
function __uiApplyRes(c, res) {
  var pk = __uiResProps(res), props = pk.props;
  // Ranges before values, items before selections.
  if (props.minvalue !== undefined) c.minvalue = __num(props.minvalue);
  if (props.maxvalue !== undefined) c.maxvalue = __num(props.maxvalue);
  if (props.items instanceof Array && c.items) for (const elem of props.items) c.add("item", elem);
  for (var k in props) {
    if (__uiResSkip[k]) continue;
    var v = props[k];
    if (__uiSizes[k] && v instanceof Array) v = __dim(v[0], v[1]);
    c[k] = v;
  }
  for (const elem of pk.kids) {
    var child = __uiAddRes(c, elem[1]);
    child.__key = elem[0];
    c[elem[0]] = child;
  }
  return c;
}

// add() with a resource string (or a parsed element): the new control.
function __uiAddRes(parent, spec) {
  var res = typeof spec === "string" ? __uiParseRes(spec) : spec;
  var p = __uiResProps(res).props;
  var t = String(res.type).toLowerCase();
  var valued = t === "dropdownlist" || t === "listbox" || t === "slider" || t === "scrollbar" || t === "progressbar";
  var c = parent.add(t, p.bounds, valued ? undefined : p.text, p.properties);
  if (valued && p.text !== undefined) c.text = String(p.text);
  return __uiApplyRes(c, res);
}
Window.alert = function (s) { alert(s); };
Window.confirm = function (s) { return confirm(s); };
Window.prompt = function (m, d) { return prompt(m, d); };
Window.find = function (type, title) {
  for (const elem of __uiWins) if (elem.text === title) return elem;
  return null;
};

function Panel() {}
function Group() {}
function Button() {}
function IconButton() {}
function StaticText() {}
function EditText() {}
function Checkbox() {}
function RadioButton() {}
function Slider() {}
function Scrollbar() {}
function Progressbar() {}
function DropDownList() {}
function ListBox() {}
function TabbedPanel() {}
function Tab() {}
// ScriptUIImage: an image file (`src`) or embedded image bytes (`data`, a binary string as in
// resource-string images: one character per byte).
function ScriptUIImage(src, data, name) {
  this.__img = true;
  this.src = src || null;
  this.data = data || null;
  this.name = name || (src ? String(src).replace(/^.*[\\/]/, "") : "");
  this.pathname = src || "";
  this.format = data ? (data.charCodeAt(0) === 0x89 ? "png" : "jpeg") : String(src || "").replace(/^.*\./, "").toLowerCase();
}
ScriptUIImage.prototype.toString = function () { return "[object ScriptUIImage]"; };
function ListItem() {}

// Anything ScriptUI accepts as an image: a ScriptUIImage, a File, a path (or resource name), or
// a binary string holding PNG / JPEG bytes.
function __uiImage(v) {
  if (v === undefined || v === null || typeof v === "number" || typeof v === "boolean") return null;
  if (v.__img) return v;
  if (v.fsName !== undefined) return new ScriptUIImage(String(v.fsName), null, String(v.name || ""));
  var s = String(v);
  if ((s.charCodeAt(0) === 0x89 && s.substr(1, 3) === "PNG") || (s.charCodeAt(0) === 0xff && s.charCodeAt(1) === 0xd8)) return new ScriptUIImage(null, s, "");
  return s ? new ScriptUIImage(s, null, "") : null;
}
function __uiImageJson(v) {
  var img = __uiImage(v);
  return img ? { src: img.src, data: img.data, name: img.name } : null;
}

var ScriptUI = {
  version: "6.2.2",
  frameworkName: "EffectCraft",
  environment: { keyboardState: { shiftKey: false, ctrlKey: false, altKey: false, metaKey: false, keyName: "" } },
  FontStyle: { REGULAR: 0, BOLD: 1, ITALIC: 2, BOLDITALIC: 3 },
  BrushType: { SOLID_COLOR: 0, THEME_COLOR: 1 },
  PenType: { SOLID_COLOR: 0, THEME_COLOR: 1 },
  Alignment: { LEFT: "left", RIGHT: "right", TOP: "top", BOTTOM: "bottom", CENTER: "center", FILL: "fill" },
  newFont: function (name, style, size) { return { name: name, style: style, size: size }; },
  newImage: function (normal) { return __uiImage(normal); },
  getResourceText: function (t) { return t; },
  events: { createEvent: function (t) { return { type: t }; } }
};

// A colour for the draw list: [r, g, b, a] in 0..1, or null for a theme colour (by name).
function __uiColor(c) {
  if (c instanceof Array && c.length >= 3) return [__num(c[0]), __num(c[1]), __num(c[2]), c.length > 3 ? __num(c[3]) : 1];
  return null;
}
function __uiFontSize(f, g) {
  var x = f?.size !== undefined ? Number(f.size) : g?.font && g.font.size !== undefined ? Number(g.font.size) : 12;
  return x > 0 ? x : 12;
}

// ScriptUIGraphics: what onDraw handlers paint with. Paths and paint calls are recorded into the
// control's draw list (`draw` in the window snapshot), which frontends paint.
function __uiGraphics() {
  return {
    BrushType: ScriptUI.BrushType,
    PenType: ScriptUI.PenType,
    font: ScriptUI.newFont("dialog", "REGULAR", 12),
    foregroundColor: null, backgroundColor: null, disabledForegroundColor: null, disabledBackgroundColor: null,
    currentPath: [],
    currentPoint: [0, 0],
    __rec: null,
    __push: function (op) { if (this.__rec) this.__rec.push(op); },
    newBrush: function (t, c) { return { type: t, color: __uiColor(c), theme: typeof c === "string" ? c : "" }; },
    newPen: function (t, c, w) { return { type: t, color: __uiColor(c), theme: typeof c === "string" ? c : "", lineWidth: w === undefined ? 1 : __num(w) }; },
    newPath: function () { this.currentPath = []; return this.currentPath; },
    moveTo: function (x, y) { this.currentPath.push({ k: "M", x: __num(x), y: __num(y) }); this.currentPoint = [__num(x), __num(y)]; },
    lineTo: function (x, y) { this.currentPath.push({ k: "L", x: __num(x), y: __num(y) }); this.currentPoint = [__num(x), __num(y)]; },
    closePath: function () { this.currentPath.push({ k: "Z" }); },
    rectPath: function (x, y, w, h) {
      if (x instanceof Array) { h = x[3] - x[1]; w = x[2] - x[0]; y = x[1]; x = x[0]; }
      this.currentPath.push({ k: "R", x: __num(x), y: __num(y), w: __num(w), h: __num(h) });
      return this.currentPath;
    },
    ellipsePath: function (x, y, w, h) {
      if (x instanceof Array) { h = x[3] - x[1]; w = x[2] - x[0]; y = x[1]; x = x[0]; }
      this.currentPath.push({ k: "E", x: __num(x), y: __num(y), w: __num(w), h: __num(h) });
      return this.currentPath;
    },
    fillPath: function (brush, path) {
      this.__push({ op: "fill", color: brush ? brush.color : null, path: (path || this.currentPath).slice() });
    },
    strokePath: function (pen, path) {
      this.__push({ op: "stroke", color: pen ? pen.color : null, width: pen?.lineWidth !== undefined ? pen.lineWidth : 1, path: (path || this.currentPath).slice() });
    },
    drawString: function (text, pen, x, y, font) {
      var f = font || this.font;
      this.__push({ op: "text", text: String(text), color: pen ? pen.color : null, x: __num(x || 0), y: __num(y || 0), size: __uiFontSize(f, this), style: String(f?.style !== undefined ? f.style : "") });
    },
    drawOSControl: function () { this.__push({ op: "os" }); },
    drawImage: function (img, x, y, w, h) { this.__push({ op: "image", image: __uiImageJson(img), x: __num(x || 0), y: __num(y || 0), w: w === undefined ? 0 : __num(w), h: h === undefined ? 0 : __num(h) }); },
    drawFocusRing: function () {},
    measureString: function (s, font, boundaryWidth) {
      var size = __uiFontSize(font, this);
      var lines = String(s).split("\n");
      var cw = size * 0.58, w = 0;
      for (const elem of lines) w = Math.max(w, Math.ceil(elem.length * cw));
      var n = lines.length;
      if (boundaryWidth > 0 && w > boundaryWidth) {
        n = Math.ceil(w / boundaryWidth) + lines.length - 1;
        w = boundaryWidth;
      }
      return __dim(w, Math.ceil(size * 1.25) * n);
    }
  };
}

// Run the onDraw handlers of a window's controls (after laying it out, so `size` is known),
// recording what they paint.
function __uiRunDraws(win) {
  var drawn = [];
  (function walk(c) {
    if (typeof c.onDraw === "function") drawn.push(c);
    for (var i = 0; i < (c.children || []).length; i++) walk(c.children[i]);
  })(win);
  if (!drawn.length) return;
  __uiLayout(win);
  for (const elem of drawn) {
    var c = elem;
    c.__draw = [];
    c.graphics.__rec = c.__draw;
    try {
      c.onDraw.call(c, { type: "draw", target: c });
    } catch (e) {
      try { writeLn("onDraw: " + (e?.message ? e.message : e)); } catch (e2) {}
    }
    c.graphics.__rec = null;
  }
}

function __uiInit(o, type, win, parent, bounds, text, props) {
  o.type = type;
  o.window = win || o;
  o.parent = parent || null;
  o.__id = win ? win.__next++ : 0;
  if (!win) {
    o.__next = 1;
    o.__wid = __uiNewId();
  }
  o.properties = props || {};
  // Image controls and icon buttons take an image where other controls take their text.
  if ((type === "image" || type === "iconbutton") && __uiImage(text)) {
    o.image = __uiImage(text);
    text = "";
  }
  o.text = text === undefined || text === null ? "" : String(text);
  o.enabled = true;
  o.visible = true;
  o.helpTip = "";
  o.children = [];
  o.__ub = __toBounds(bounds);
  o.__lb = [0, 0, 0, 0];
  o.preferredSize = __dim(-1, -1);
  o.minimumSize = __dim(0, 0);
  o.maximumSize = __dim(10000, 10000);
  o.alignment = null;
  o.graphics = __uiGraphics();
  o.__listeners = {};
  if (o.properties.name) o.name = o.properties.name;
  if (__uiContainers[type]) {
    o.orientation = type === "group" ? "row" : type === "tabbedpanel" ? "stack" : "column";
    o.alignChildren = type === "group" ? ["center", "center"] : ["center", "top"];
    o.spacing = 10;
    o.margins = type === "window" ? [15, 15, 15, 15] : type === "panel" ? [10, 15, 10, 10] : type === "tab" ? [10, 10, 10, 10] : [0, 0, 0, 0];
    var self = o;
    o.layout = {
      layout: function () { __uiLayout(self.window); },
      resize: function () { __uiLayout(self.window); },
      margins: o.margins,
      spacing: o.spacing
    };
  }
  if (type === "checkbox" || type === "radiobutton") o.value = !!o.properties.value;
  if (type === "slider" || type === "scrollbar" || type === "progressbar") {
    o.value = 0;
    o.minvalue = 0;
    o.maxvalue = 100;
  }
  if (type === "edittext") {
    o.characters = 0;
    o.textselection = "";
    o.active = false;
  }
  if (type === "dropdownlist" || type === "listbox") {
    o.items = [];
    o.__sel = [];
    var it = o.properties.items;
    if (it) for (var i = 0; i < it.length; i++) o.add("item", it[i]);
  }
  if (type === "tabbedpanel") o.__sel = [];
  return o;
}

function __uiProto() {
  return {
    add: function (type, bounds, text, props) {
      var t = String(type).toLowerCase();
      if (t.indexOf("{") >= 0) {
        if (!__uiContainers[this.type]) throw __err("add(): a " + this.type + " can't contain controls");
        return __uiAddRes(this, String(type));
      }
      if (t === "item" || t === "separator") return this.__addItem(t === "separator" ? "-" : bounds);
      if (!__uiContainers[this.type]) throw __err("add(): a " + this.type + " can't contain controls");
      var ctor = __uiTypes[t];
      if (!ctor || t === "window") throw __err("add(): unknown control type " + type);
      var c = Object.create(__ecGlobal[ctor].prototype);
      // Value arguments after `text`: slider (value, min, max), progressbar (value, max), lists (items).
      var arg = t === "image" || t === "iconbutton" ? text : typeof text === "object" && text !== null && !(text instanceof Array) ? "" : text;
      __uiInit(c, t, this.window, this, bounds, arg, props);
      if ((t === "dropdownlist" || t === "listbox") && text instanceof Array) {
        for (const elem of text) c.add("item", elem);
        c.text = "";
      }
      if (t === "slider" || t === "scrollbar") {
        c.text = "";
        c.value = text === undefined ? 0 : __num(text);
        if (arguments.length > 3 && typeof props === "number") {
          c.minvalue = props;
          c.maxvalue = arguments[4] === undefined ? 100 : __num(arguments[4]);
          c.properties = arguments[5] || {};
        }
      }
      if (t === "progressbar") {
        c.text = "";
        c.value = text === undefined ? 0 : __num(text);
        if (typeof props === "number") {
          c.maxvalue = props;
          c.properties = arguments[4] || {};
        }
      }
      if (t === "checkbox" || t === "radiobutton") c.value = false;
      if (t === "edittext" && c.properties.multiline) c.__multiline = true;
      if (t === "edittext" && c.properties.readonly) c.__readonly = true;
      if (c.properties?.name) c.name = c.properties.name;
      this.children.push(c);
      if (t === "tab" && this.type === "tabbedpanel" && this.__sel.length === 0) this.__sel = [this.children.length - 1];
      return c;
    },
    remove: function (what) {
      var list = this.items && this.type !== "tabbedpanel" ? this.items : this.children;
      var i = typeof what === "number" ? what : list.indexOf(what);
      if (i < 0 || i >= list.length) return;
      list.splice(i, 1);
      if (this.items) {
        for (var k = 0; k < this.items.length; k++) this.items[k].index = k;
        this.__sel = this.__sel.filter(function (s) { return s !== i; }).map(function (s) { return s > i ? s - 1 : s; });
      }
    },
    removeAll: function () {
      if (this.items) {
        this.items = [];
        this.__sel = [];
      } else this.children = [];
    },
    find: function (text) {
      var list = this.items || this.children;
      for (var i = 0; i < list.length; i++) if (list[i].text === text) return list[i];
      return null;
    },
    __addItem: function (text) {
      if (!this.items) throw __err("add(\"item\"): only lists have items");
      var it = Object.create(ListItem.prototype);
      it.text = String(text === undefined ? "" : text);
      it.type = text === "-" ? "separator" : "item";
      it.index = this.items.length;
      it.parent = this;
      it.image = null;
      it.checked = false;
      this.items.push(it);
      return it;
    },
    show: function () {
      if (!this.__root) {
        this.visible = true;
        return;
      }
      this.__shown = true;
      this.__closed = false;
      this.__fire("onShow");
      if (this.__kind === "dialog") {
        this.__modal = true;
        var r = __uiModal(this.__wid);
        this.__modal = false;
        return r === null ? undefined : r;
      }
      return undefined;
    },
    hide: function () {
      if (this.__root) {
        this.close(2);
      } else this.visible = false;
    },
    close: function (result) {
      if (!this.__root) return;
      if (this.__fire("onClose") === false) return;
      this.__closed = true;
      this.__shown = false;
      this.__result = result === undefined ? 0 : result;
      if (this.__modal) __uiResults[this.__wid] = this.__result;
    },
    center: function () {},
    notify: function (ev) {
      var e = ev || (this.type === "edittext" || this.type === "slider" || this.type === "dropdownlist" || this.type === "listbox" ? "onChange" : "onClick");
      if (e.indexOf("on") !== 0) e = "on" + e.charAt(0).toUpperCase() + e.slice(1);
      this.__fire(e);
    },
    // `capture`: the listener runs in the capture phase of mouse events (on the way down from
    // the window to the target) instead of the bubble phase.
    addEventListener: function (name, fn, capture) {
      var k = String(name).toLowerCase();
      if (capture) {
        if (!this.__capture) this.__capture = Object.create(null);
        (this.__capture[k] = this.__capture[k] || []).push(fn);
        return;
      }
      (this.__listeners[k] = this.__listeners[k] || []).push(fn);
    },
    removeEventListener: function (name, fn, capture) {
      var k = String(name).toLowerCase();
      var m = capture ? this.__capture : this.__listeners;
      if (m?.[k]) m[k] = m[k].filter(function (f) { return f !== fn; });
    },
    dispatchEvent: function (e) { this.__fire("on" + String(e.type || e).charAt(0).toUpperCase() + String(e.type || e).slice(1)); },
    __fire: function (name) {
      var r;
      if (typeof this[name] === "function") r = this[name].call(this);
      var l = this.__listeners[name.slice(2).toLowerCase()];
      if (l) for (const elem of l) elem.call(this, { type: name.slice(2).toLowerCase(), target: this });
      return r;
    },
    __handlers: function () {
      var out = [];
      var names = ["onClick", "onChange", "onChanging", "onDoubleClick", "onClose", "onShow", "onActivate", "onDeactivate", "onResize", "onDraw"];
      for (const elem of names) {
        var n = elem;
        if (typeof this[n] === "function" || (this.__listeners[n.slice(2).toLowerCase()] || []).length) out.push(n);
      }
      return out;
    },
    toString: function () { return "[object " + (__uiTypes[this.type] || "Window") + "]"; }
  };
}

(function () {
  var all = ["Window", "Panel", "Group", "Button", "IconButton", "StaticText", "EditText", "Checkbox", "RadioButton", "Slider",
    "Scrollbar", "Progressbar", "DropDownList", "ListBox", "TabbedPanel", "Tab", "ScriptUIImage"];
  for (const elem of all) {
    var C = __ecGlobal[elem];
    var p = __uiProto();
    for (var k in p) C.prototype[k] = p[k];
    Object.defineProperty(C.prototype, "bounds", {
      get: function () { var b = this.__ub || this.__lb; return __bounds4(b[0], b[1], b[2], b[3]); },
      set: function (v) { this.__ub = __toBounds(v); },
      configurable: true
    });
    Object.defineProperty(C.prototype, "size", {
      get: function () { var b = this.bounds; return __dim(b[2] - b[0], b[3] - b[1]); },
      set: function (v) {
        var b = this.bounds;
        var w = v.width !== undefined ? v.width : v[0], h = v.height !== undefined ? v.height : v[1];
        this.__ub = [b[0], b[1], b[0] + __num(w), b[1] + __num(h)];
      },
      configurable: true
    });
    Object.defineProperty(C.prototype, "location", {
      get: function () { var b = this.bounds; return __dim(b[0], b[1]); },
      set: function (v) {
        var b = this.bounds;
        var x = v.x !== undefined ? v.x : v[0], y = v.y !== undefined ? v.y : v[1];
        this.__ub = [__num(x), __num(y), __num(x) + b[2] - b[0], __num(y) + b[3] - b[1]];
      },
      configurable: true
    });
  }
  var sel = {
    get: function () {
      var self = this;
      if (this.type === "tabbedpanel") return this.__sel.length ? this.children[this.__sel[0]] : null;
      var items = this.__sel.map(function (i) { return self.items[i]; });
      if (this.properties.multiselect) return items.length ? items : null;
      return items.length ? items[0] : null;
    },
    set: function (v) {
      var self = this;
      var list = this.type === "tabbedpanel" ? this.children : this.items;
      var idx = function (x) { return typeof x === "number" ? x : list.indexOf(x); };
      if (v === null || v === undefined) this.__sel = [];
      else if (v instanceof Array) this.__sel = v.map(idx).filter(function (i) { return i >= 0 && i < list.length; });
      else {
        var i = idx(v);
        this.__sel = i >= 0 && i < list.length ? [i] : [];
      }
      if (this.items) for (var k = 0; k < this.items.length; k++) this.items[k].selected = this.__sel.indexOf(k) >= 0;
    },
    configurable: true
  };
  Object.defineProperty(DropDownList.prototype, "selection", sel);
  Object.defineProperty(ListBox.prototype, "selection", sel);
  Object.defineProperty(TabbedPanel.prototype, "selection", sel);
  Object.defineProperty(ListItem.prototype, "toString", { value: function () { return this.text; } });
})();

// A dockable ScriptUI panel (scripts in the ScriptUI Panels folder run with `this` = it).
function __uiDockPanel(title) {
  var p = Object.create(Panel.prototype);
  __uiInit(p, "panel", null, null, undefined, title, {});
  p.type = "panel";
  p.__kind = "panel";
  p.__root = true;
  p.__shown = true;
  p.margins = [10, 10, 10, 10];
  __uiWins.push(p);
  return p;
}

function __uiAlign(v) {
  if (v === null || v === undefined) return [];
  if (v instanceof Array) return v.map(function (x) { return String(x).toLowerCase(); });
  return [String(v).toLowerCase()];
}
function __uiMargins(m) {
  if (m === null || m === undefined) return null;
  if (typeof m === "number") return [m, m, m, m];
  if (m.length === 4) return [m[0], m[1], m[2], m[3]];
  return null;
}
function __uiSize(d) {
  if (!d) return null;
  var w = d.width !== undefined ? d.width : d[0], h = d.height !== undefined ? d.height : d[1];
  w = Number(w);
  h = Number(h);
  if (!(w > 0) && !(h > 0)) return null;
  return [w > 0 ? w : 0, h > 0 ? h : 0];
}

// ScriptUI mouse events: delivered to `addEventListener` listeners of the control under the
// pointer and its containers (capture, target, bubble), as the JavaScript Tools Guide documents.
var __uiMouseKinds = ["mousedown", "mouseup", "mousemove", "mouseover", "mouseout", "click"];

function __uiMouseListeners(c, type, capture) {
  var m = capture ? c.__capture : c.__listeners;
  if (!m || !Object.prototype.hasOwnProperty.call(m, type)) return [];
  var l = m[type];
  return l instanceof Array ? l.slice() : [];
}

// The mouse events that reach a listener at `c`: its own, and its containers' (`inherited`).
function __uiMouseWanted(c, inherited) {
  var out = inherited.slice();
  for (const elem of __uiMouseKinds) {
    var t = elem;
    if (out.indexOf(t) < 0 && (__uiMouseListeners(c, t, false).length || __uiMouseListeners(c, t, true).length)) out.push(t);
  }
  return out;
}

// A ScriptUI MouseEvent. `v`: {x, y (relative to the target control), button (0 left, 1 middle,
// 2 right), detail (click count), screenX, screenY, altKey, ctrlKey, metaKey, shiftKey}.
function __uiMouseEvent(type, target, v) {
  var num = function (x, d) {
    x = Number(x);
    return isFinite(x) ? x : d;
  };
  var keys = { alt: !!v.altKey, control: !!v.ctrlKey, ctrl: !!v.ctrlKey, meta: !!v.metaKey, shift: !!v.shiftKey };
  return {
    type: type,
    target: target,
    currentTarget: target,
    eventPhase: "none",
    bubbles: true,
    cancelable: true,
    captures: true,
    view: target.window,
    relatedTarget: undefined,
    timeStamp: new Date(),
    detail: num(v.detail, type === "mousemove" || type === "mouseover" || type === "mouseout" ? 0 : 1),
    button: num(v.button, 0),
    clientX: num(v.x, 0),
    clientY: num(v.y, 0),
    screenX: num(v.screenX, 0),
    screenY: num(v.screenY, 0),
    altKey: keys.alt,
    ctrlKey: keys.ctrl,
    metaKey: keys.meta,
    shiftKey: keys.shift,
    defaultPrevented: false,
    __stopped: false,
    stopPropagation: function () { this.__stopped = true; },
    preventDefault: function () { this.defaultPrevented = true; },
    getModifierState: function (k) { return !!keys[String(k).toLowerCase()]; },
    toString: function () { return "[object MouseEvent]"; }
  };
}

// Deliver a mouse event to `target`: capture listeners from the window down, the target's own
// listeners, then bubble listeners back up. stopPropagation() ends it after the current control.
function __uiMouseFire(target, type, v) {
  var path = [];
  // A tree built by add() is shallow; the bound keeps a script's own `parent` cycle harmless.
  for (var p = target; p && path.length < 256; p = p.parent) path.unshift(p);
  var e = __uiMouseEvent(type, target, v);
  var run = function (c, phase, capture) {
    var l = __uiMouseListeners(c, type, capture);
    e.currentTarget = c;
    e.eventPhase = phase;
    for (const elem of l) if (typeof elem === "function") elem.call(c, e);
  };
  var n = path.length - 1;
  for (var i = 0; i < n; i++) {
    run(path[i], "capture", true);
    if (e.__stopped) return e;
  }
  run(target, "target", true);
  run(target, "target", false);
  for (var j = n - 1; j >= 0 && !e.__stopped; j--) run(path[j], "bubble", false);
  return e;
}

function __uiJson(c, inherited) {
  var mouse = __uiMouseWanted(c, inherited || []);
  var ps = __uiSize(c.preferredSize);
  if (c.type === "edittext" && c.characters > 0) ps = [c.characters * 7 + 12, ps ? ps[1] : 0];
  var o = {
    id: c.__id,
    type: c.type,
    name: String(c.name || c.properties.name || c.__key || ""),
    text: String(c.text === undefined || c.text === null ? "" : c.text),
    enabled: c.enabled !== false,
    visible: c.visible !== false,
    helpTip: String(c.helpTip || ""),
    handlers: c.__handlers(),
    children: []
  };
  if (ps) o.preferredSize = ps;
  if (mouse.length) o.mouse = mouse;
  if (c.__ub && !c.__root) o.fixedBounds = c.__ub;
  if (c.__root && c.__ub) o.fixedBounds = [0, 0, c.__ub[2] - c.__ub[0], c.__ub[3] - c.__ub[1]];
  if (__uiContainers[c.type]) {
    o.orientation = String(c.orientation || "").toLowerCase();
    o.alignChildren = __uiAlign(c.alignChildren);
    var m = __uiMargins(c.margins);
    if (m) o.margins = m;
    if (typeof c.spacing === "number") o.spacing = c.spacing;
    var kids = c.children;
    for (const elem of kids) o.children.push(__uiJson(elem, mouse));
  }
  if (c.alignment !== null && c.alignment !== undefined) o.alignment = __uiAlign(c.alignment);
  if (c.type === "checkbox" || c.type === "radiobutton") o.checked = !!c.value;
  if (c.type === "slider" || c.type === "scrollbar" || c.type === "progressbar") {
    o.value = Number(c.value) || 0;
    o.min = Number(c.minvalue) || 0;
    o.max = Number(c.maxvalue);
    if (Number.isNaN(o.max)) o.max = 100;
  }
  if (c.items) {
    o.items = c.items.map(function (it) { return String(it.text); });
    o.selection = c.__sel.slice();
  }
  if (c.type === "tabbedpanel") o.activeTab = c.__sel.length ? c.__sel[0] : 0;
  if (c.__draw && typeof c.onDraw === "function") o.draw = c.__draw;
  if ((c.type === "image" || c.type === "iconbutton") && c.image) o.image = __uiImageJson(c.image);
  if (c.type === "edittext") {
    o.multiline = !!c.__multiline;
    o.readOnly = !!c.__readonly;
  }
  return o;
}

// The open windows of this script, for the host.
function __uiSnapshot() {
  var out = [];
  for (const elem of __uiWins) {
    var w = elem;
    if (w.__shown && !w.__closed) __uiRunDraws(w);
    out.push({ id: w.__wid, kind: w.__kind, title: String(w.text || ""), visible: !!w.__shown && !w.__closed, modal: !!w.__modal, root: __uiJson(w) });
  }
  // Closed windows are forgotten.
  __uiWins = __uiWins.filter(function (w) { return !w.__closed; });
  return JSON.stringify(out);
}

function __uiOpenCount() {
  var n = 0;
  for (const elem of __uiWins) if (elem.__shown && !elem.__closed) n++;
  return n;
}

function __uiWin(id) {
  for (const elem of __uiWins) if (elem.__wid === id) return elem;
  return null;
}
function __uiFind(c, id) {
  if (c.__id === id) return c;
  for (var i = 0; i < (c.children || []).length; i++) {
    var r = __uiFind(c.children[i], id);
    if (r) return r;
  }
  return null;
}

// layout.layout(): lay the window out now (the host does it before drawing anyway) so scripts
// can read bounds and sizes.
function __uiLayout(win) {
  var b = JSON.parse(__uiLayoutNative(JSON.stringify({ id: win.__wid, kind: win.__kind, title: String(win.text || ""), visible: true, root: __uiJson(win) })));
  // Bounds are relative to the parent container, as in ScriptUI.
  (function apply(c, ox, oy) {
    var r = b[c.__id];
    if (!r) return;
    c.__lb = [r[0] - ox, r[1] - oy, r[0] - ox + r[2], r[1] - oy + r[3]];
    for (var i = 0; i < (c.children || []).length; i++) apply(c.children[i], r[0], r[1]);
  })(win, 0, 0);
}

// A user action from the frontend.
function __uiDispatch(winId, id, kind, valueJson) {
  var w = __uiWin(winId);
  if (!w) return;
  var v = valueJson === undefined || valueJson === "" ? null : JSON.parse(valueJson);
  if (kind === "close") {
    w.close(v === null ? 2 : v);
    return;
  }
  var c = __uiFind(w, id);
  if (!c || c.enabled === false) return;
  var radio = function () {
    var sib = c.parent ? c.parent.children : [];
    for (const elem of sib) if (elem !== c && elem.type === "radiobutton") elem.value = false;
    c.value = true;
  };
  // A mouse click carries the pointer's details ({x, y, button, detail, …keys}).
  var mouse = v !== null && typeof v === "object" && !(v instanceof Array) ? v : null;
  if (kind !== "click" && __uiMouseKinds.indexOf(kind) >= 0) {
    __uiMouseFire(c, kind, mouse || {});
    return;
  }
  if (kind === "click") {
    if (c.type === "checkbox") c.value = !c.value;
    else if (c.type === "radiobutton") radio();
    else if (c.type === "tab" && c.parent) {
      c.parent.selection = c;
      c.parent.__fire("onChange");
      if (mouse) __uiMouseFire(c, "click", mouse);
      return;
    }
    var handled = c.__handlers().indexOf("onClick") >= 0;
    if (mouse) {
      if (typeof c.onClick === "function") c.onClick.call(c);
      __uiMouseFire(c, "click", mouse);
    } else c.__fire("onClick");
    // Dialog buttons without handlers: OK closes with 1, Cancel with 2.
    if (!handled && c.type === "button" && w.__kind === "dialog") {
      var n = String(c.name || c.properties.name || c.text).toLowerCase();
      if (c === w.defaultElement || n === "ok") w.close(1);
      else if (c === w.cancelElement || n === "cancel") w.close(2);
    }
    return;
  }
  // change / changing (live: each keystroke of an edit text, each step of a slider drag)
  var before = c.type === "edittext" ? c.text : c.value;
  if (c.type === "edittext") c.text = v === null ? "" : String(v);
  else if (c.type === "statictext") c.text = v === null ? "" : String(v);
  else if (c.type === "slider" || c.type === "scrollbar" || c.type === "progressbar") c.value = Number(v);
  else if (c.type === "checkbox") c.value = !!v;
  else if (c.type === "radiobutton") { if (v) radio(); else c.value = false; }
  else if (c.type === "dropdownlist" || c.type === "listbox" || c.type === "tabbedpanel") c.selection = v;
  var now = c.type === "edittext" ? c.text : c.value;
  // onChanging already ran for live updates of the same value.
  if ((c.type === "edittext" || c.type === "slider" || c.type === "scrollbar") && (kind === "changing" || now !== before)) c.__fire("onChanging");
  if (kind === "change") {
    if (c.type === "checkbox" || c.type === "radiobutton") c.__fire("onClick");
    else c.__fire("onChange");
  }
}
