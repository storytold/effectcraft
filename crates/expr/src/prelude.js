// EffectCraft expression runtime: the After Effects expression object model and global
// functions, written from the public Expression Language Reference (behaviour only).
//
// Natives (Rust): __h(op, ...) host data request; __wig(...) wiggle offsets; __noise(x, y, z);
// __rnd(...) seeded random. This file is evaluated once per runtime and is NOT passed through
// the array-maths rewriter, so it uses plain JS operators.

var time = 0;
var value = 0;
var __S = null;
var __rc = 0;
var __seed = 0;
var __timeless = false;
// Text Expression Selector inputs (1, 1, [100, 100, 100] outside a selector).
var textIndex = 1;
var textTotal = 1;
var selectorValue = [100, 100, 100];

// Request ops (must match runtime.rs).
var __COMP = 0, __COMPNAME = 1, __LAYER = 2, __LINFO = 3, __CHILD = 4, __PINFO = 5, __VALUE = 6, __XFORM = 7, __RECT = 8, __MARKERS = 9, __DOC = 10;
var __FOOT = 11, __FDATA = 12, __SAMPLE = 13, __PROJ = 14, __SOURCE = 15;

function __begin(t, v, c, l, path, uid, idx, fd, ti, tt, sv) {
  time = t;
  value = v;
  textIndex = ti;
  textTotal = tt;
  selectorValue = sv;
  __S = { c: c, l: l, path: path, uid: uid, idx: idx, fd: fd, comp: null, layer: null, prop: null, vars: {} };
  __rc = 0;
  __seed = 0;
  __timeless = false;
  __projectInfo = null;
}

function __finish(r) {
  // Rethrow from this frame so the runtime's stack unwinds (see `Runtime::script`).
  try {
    return __result(r);
  } catch (e) {
    throw e;
  }
}

function __result(r) {
  r = __v(r);
  if (r !== null && typeof r === 'object' && r.__isLayer) return r.index;
  if (r !== null && typeof r === 'object' && r.__isTextStyle) return r.__out();
  // `[temp, temp]` where temp is a property (pick-whip 1D → 2D): read the values here, while
  // host requests can still be answered and the script re-run.
  if (Array.isArray(r)) return r.map(function (x) { return __v(x); });
  return r;
}

function __err(msg) {
  throw new Error(msg);
}

// ---------------------------------------------------------------------------------------------
// Array maths (targets of the operator rewrite).

function __v(x) {
  if (x !== null && typeof x === 'object' && x.__isProp === true) return x.value;
  return x;
}

function __bin(a, b, f) {
  a = __v(a);
  b = __v(b);
  var aa = Array.isArray(a), ba = Array.isArray(b), i, r;
  if (aa && ba) {
    var n = Math.max(a.length, b.length);
    r = new Array(n);
    for (i = 0; i < n; i++) r[i] = f(i < a.length ? a[i] : 0, i < b.length ? b[i] : 0);
    return r;
  }
  if (aa && (typeof b === 'number' || typeof b === 'boolean')) {
    r = new Array(a.length);
    for (i = 0; i < a.length; i++) r[i] = f(a[i], b);
    return r;
  }
  if (ba && (typeof a === 'number' || typeof a === 'boolean')) {
    r = new Array(b.length);
    for (i = 0; i < b.length; i++) r[i] = f(a, b[i]);
    return r;
  }
  return f(a, b);
}

function __fadd(x, y) { return x + y; }
function __fsub(x, y) { return x - y; }
function __fmul(x, y) { return x * y; }
function __fdiv(x, y) { return x / y; }

function __add(a, b) {
  if (typeof a === 'number' && typeof b === 'number') return a + b;
  if (typeof a === 'string' || typeof b === 'string') return __v(a) + __v(b);
  var x = __v(a), y = __v(b);
  if (typeof x === 'string' || typeof y === 'string') return x + y;
  if ((Array.isArray(x) && typeof y === 'string') || (Array.isArray(y) && typeof x === 'string')) return x + y;
  return __bin(x, y, __fadd);
}
function __sub(a, b) {
  if (typeof a === 'number' && typeof b === 'number') return a - b;
  return __bin(a, b, __fsub);
}
function __mul(a, b) {
  if (typeof a === 'number' && typeof b === 'number') return a * b;
  return __bin(a, b, __fmul);
}
function __div(a, b) {
  if (typeof a === 'number' && typeof b === 'number') return a / b;
  return __bin(a, b, __fdiv);
}
function __neg(a) {
  if (typeof a === 'number') return -a;
  a = __v(a);
  if (Array.isArray(a)) return a.map(function (x) { return -x; });
  return -a;
}

// ---------------------------------------------------------------------------------------------
// Vector maths.

function add(a, b) { return __bin(a, b, __fadd); }
function sub(a, b) { return __bin(a, b, __fsub); }
function mul(a, b) { return __bin(a, b, __fmul); }
function div(a, b) { return __bin(a, b, __fdiv); }

function clamp(v, a, b) {
  v = __v(v); a = __v(a); b = __v(b);
  var lo = function (x, y) { return Math.min(Math.max(x, Math.min(y[0], y[1])), Math.max(y[0], y[1])); };
  if (Array.isArray(v)) {
    return v.map(function (x, i) {
      return lo(x, [Array.isArray(a) ? a[i] : a, Array.isArray(b) ? b[i] : b]);
    });
  }
  return lo(v, [a, b]);
}

function dot(a, b) {
  a = __v(a); b = __v(b);
  var s = 0;
  for (var i = 0; i < Math.min(a.length, b.length); i++) s += a[i] * b[i];
  return s;
}

function cross(a, b) {
  a = __v(a); b = __v(b);
  var a2 = a[2] || 0, b2 = b[2] || 0;
  return [a[1] * b2 - a2 * b[1], a2 * b[0] - a[0] * b2, a[0] * b[1] - a[1] * b[0]];
}

function length(a, b) {
  a = __v(a);
  if (b !== undefined) a = sub(a, __v(b));
  if (!Array.isArray(a)) return Math.abs(a);
  var s = 0;
  for (var i = 0; i < a.length; i++) s += a[i] * a[i];
  return Math.sqrt(s);
}

function normalize(a) {
  a = __v(a);
  var l = length(a);
  return l === 0 ? a.map(function () { return 0; }) : div(a, l);
}

function lookAt(from, at) {
  var d = normalize(sub(__v(at), __v(from)));
  var x = d[0], y = d[1], z = d[2] || 0;
  var rx = -Math.asin(Math.max(-1, Math.min(1, y))) * 180 / Math.PI;
  var ry = Math.atan2(x, z) * 180 / Math.PI;
  return [rx, ry, 0];
}

function degreesToRadians(d) { return d * Math.PI / 180; }
function radiansToDegrees(r) { return r * 180 / Math.PI; }

// ---------------------------------------------------------------------------------------------
// Interpolation.

function __interp(shape, args) {
  var t, tMin, tMax, v1, v2;
  if (args.length <= 3) {
    t = args[0]; tMin = 0; tMax = 1; v1 = args[1]; v2 = args[2];
  } else {
    t = args[0]; tMin = args[1]; tMax = args[2]; v1 = args[3]; v2 = args[4];
  }
  t = __v(t); v1 = __v(v1); v2 = __v(v2);
  var u;
  if (tMax === tMin) u = t >= tMax ? 1 : 0;
  else u = (t - tMin) / (tMax - tMin);
  u = Math.max(0, Math.min(1, u));
  u = shape(u);
  if (Array.isArray(v1) || Array.isArray(v2)) return add(v1, mul(sub(v2, v1), u));
  return v1 + (v2 - v1) * u;
}

function __lin(u) { return u; }
function __ease(u) { return u * u * (3 - 2 * u); }
function __easeIn(u) { return u * u * (2 - u); }
function __easeOut(u) { var w = 1 - u; return 1 - w * w * (2 - w); }

function linear() { return __interp(__lin, arguments); }
function ease() { return __interp(__ease, arguments); }
function easeIn() { return __interp(__easeIn, arguments); }
function easeOut() { return __interp(__easeOut, arguments); }

// ---------------------------------------------------------------------------------------------
// Random numbers and noise.

function seedRandom(offset, timeless) {
  __seed = offset;
  __timeless = !!timeless;
}

function __r01() {
  return __rnd(__S.idx, __S.uid, __seed, __timeless, time, __rc++);
}

function __scaleRand(r, args) {
  if (args.length === 0) return r();
  var a = __v(args[0]);
  if (args.length === 1) {
    if (Array.isArray(a)) return a.map(function (m) { return r() * m; });
    return r() * a;
  }
  var b = __v(args[1]);
  if (Array.isArray(a) || Array.isArray(b)) {
    var n = Math.max(Array.isArray(a) ? a.length : 1, Array.isArray(b) ? b.length : 1), out = [];
    for (var i = 0; i < n; i++) {
      var lo = Array.isArray(a) ? (a[i] || 0) : a, hi = Array.isArray(b) ? (b[i] || 0) : b;
      out.push(lo + r() * (hi - lo));
    }
    return out;
  }
  return a + r() * (b - a);
}

function random() { return __scaleRand(__r01, arguments); }

function __gauss01() {
  var u1 = Math.max(__r01(), 1e-12), u2 = __r01();
  var z = Math.sqrt(-2 * Math.log(u1)) * Math.cos(2 * Math.PI * u2);
  // ~90% of results fall in [0, 1].
  return 0.5 + z * 0.304;
}

function gaussRandom() { return __scaleRand(__gauss01, arguments); }

function noise(x) {
  x = __v(x);
  if (Array.isArray(x)) return __noise(x[0] || 0, x[1] || 0, x[2] || 0);
  return __noise(x, 0, 0);
}

// ---------------------------------------------------------------------------------------------
// Time conversion.

function timeToFrames(t, fps, isDuration) {
  if (t === undefined) t = time + thisComp.displayStartTime;
  if (fps === undefined) fps = 1 / __S.fd;
  return Math.floor(t * fps + 1e-6);
}

function framesToTime(frames, fps) {
  if (fps === undefined) fps = 1 / __S.fd;
  return frames / fps;
}

function __pad(n) { return n < 10 ? '0' + n : '' + n; }

function timeToTimecode(t, base, isDuration) {
  if (t === undefined) t = time + thisComp.displayStartTime;
  if (base === undefined) base = 30;
  var neg = t < 0;
  var f = Math.floor(Math.abs(t) * base + 1e-6);
  var ff = f % base, s = Math.floor(f / base), ss = s % 60, m = Math.floor(s / 60), mm = m % 60, h = Math.floor(m / 60);
  return (neg ? '-' : '') + h + ':' + __pad(mm) + ':' + __pad(ss) + ':' + __pad(ff);
}

function timeToFeetAndFrames(t, fps, framesPerFoot, isDuration) {
  if (t === undefined) t = time + thisComp.displayStartTime;
  if (fps === undefined) fps = 1 / __S.fd;
  if (framesPerFoot === undefined) framesPerFoot = 16;
  var neg = t < 0;
  var f = Math.floor(Math.abs(t) * fps + 1e-6);
  var feet = Math.floor(f / framesPerFoot), fr = f % framesPerFoot;
  var s = String(fr);
  while (s.length < 2) s = '0' + s;
  return (neg ? '-' : '') + feet + '+' + s;
}

function timeToCurrentFormat(t, fps) {
  if (fps === undefined) fps = Math.round(1 / __S.fd);
  return timeToTimecode(t, fps);
}

function posterizeTime(fps) {
  if (!(fps > 0)) return;
  var t = Math.floor(time * fps + 1e-6) / fps;
  if (t !== time) {
    var v = thisProperty.valueAtTime(t);
    time = t;
    value = v;
  }
}

// ---------------------------------------------------------------------------------------------
// Colour.

function rgbToHsl(c) {
  c = __v(c);
  var r = c[0], g = c[1], b = c[2], a = c.length > 3 ? c[3] : 1;
  var mx = Math.max(r, g, b), mn = Math.min(r, g, b), l = (mx + mn) / 2, h = 0, s = 0;
  if (mx !== mn) {
    var d = mx - mn;
    s = l > 0.5 ? d / (2 - mx - mn) : d / (mx + mn);
    if (mx === r) h = (g - b) / d + (g < b ? 6 : 0);
    else if (mx === g) h = (b - r) / d + 2;
    else h = (r - g) / d + 4;
    h /= 6;
  }
  return [h, s, l, a];
}

function hslToRgb(c) {
  c = __v(c);
  var h = c[0], s = c[1], l = c[2], a = c.length > 3 ? c[3] : 1;
  if (s === 0) return [l, l, l, a];
  var hue = function (p, q, t) {
    if (t < 0) t += 1;
    if (t > 1) t -= 1;
    if (t < 1 / 6) return p + (q - p) * 6 * t;
    if (t < 1 / 2) return q;
    if (t < 2 / 3) return p + (q - p) * (2 / 3 - t) * 6;
    return p;
  };
  var q = l < 0.5 ? l * (1 + s) : l + s - l * s, p = 2 * l - q;
  return [hue(p, q, h + 1 / 3), hue(p, q, h), hue(p, q, h - 1 / 3), a];
}

function hexToRgb(hex) {
  var s = String(hex).replace(/^#|^0x/i, '');
  if (s.length === 3) s = s[0] + s[0] + s[1] + s[1] + s[2] + s[2];
  var n = parseInt(s.slice(0, 6), 16);
  var a = s.length >= 8 ? parseInt(s.slice(6, 8), 16) / 255 : 1;
  return [((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255, a];
}

// ---------------------------------------------------------------------------------------------
// Paths.

function __Path(pts, ins, outs, closed) {
  this.__path = true;
  this.__pts = pts; this.__in = ins; this.__out = outs; this.__closed = !!closed;
}
__Path.prototype.points = function () { return this.__pts; };
__Path.prototype.inTangents = function () { return this.__in; };
__Path.prototype.outTangents = function () { return this.__out; };
__Path.prototype.isClosed = function () { return this.__closed; };
// pointOnPath / tangentOnPath: a point (or unit tangent) at a fraction of the path's arc length.
__Path.prototype.pointOnPath = function (pct) { return __pathAt(this, pct === undefined ? 0.5 : pct, false); };
__Path.prototype.tangentOnPath = function (pct) { return __pathAt(this, pct === undefined ? 0.5 : pct, true); };
__Path.prototype.normalOnPath = function (pct) { var t = this.tangentOnPath(pct); return [t[1], -t[0]]; };

function __pathSegs(p) {
  var n = p.__pts.length, out = [];
  var m = p.__closed ? n : n - 1;
  for (var i = 0; i < m; i++) {
    var j = (i + 1) % n, a = p.__pts[i], b = p.__pts[j];
    var o = p.__out[i] || [0, 0], ii = p.__in[j] || [0, 0];
    out.push([a, [a[0] + o[0], a[1] + o[1]], [b[0] + ii[0], b[1] + ii[1]], b]);
  }
  return out;
}
function __bezAt(s, u) {
  var v = 1 - u;
  return [0, 1].map(function (k) { return v * v * v * s[0][k] + 3 * v * v * u * s[1][k] + 3 * v * u * u * s[2][k] + u * u * u * s[3][k]; });
}
function __bezDer(s, u) {
  var v = 1 - u;
  return [0, 1].map(function (k) { return 3 * v * v * (s[1][k] - s[0][k]) + 6 * v * u * (s[2][k] - s[1][k]) + 3 * u * u * (s[3][k] - s[2][k]); });
}
function __pathAt(p, pct, tangent) {
  var segs = __pathSegs(p);
  if (!segs.length) return tangent ? [1, 0] : p.__pts.length ? [p.__pts[0][0], p.__pts[0][1]] : [0, 0];
  var N = 32, table = [], total = 0;
  for (var k = 0; k < segs.length; k++) {
    var prev = __bezAt(segs[k], 0);
    for (var s = 1; s <= N; s++) {
      var q = __bezAt(segs[k], s / N);
      total += Math.sqrt((q[0] - prev[0]) * (q[0] - prev[0]) + (q[1] - prev[1]) * (q[1] - prev[1]));
      table.push([total, k, s / N]);
      prev = q;
    }
  }
  var target = Math.max(0, Math.min(1, pct)) * total;
  var i = 0;
  while (i < table.length - 1 && table[i][0] < target) i++;
  var e = table[i], seg = e[1];
  var l0 = i > 0 ? table[i - 1][0] : 0, u0 = i > 0 && table[i - 1][1] === seg ? table[i - 1][2] : 0;
  var u = e[0] > l0 ? u0 + (e[2] - u0) * (target - l0) / (e[0] - l0) : e[2];
  if (!tangent) return __bezAt(segs[seg], u);
  var d = __bezDer(segs[seg], u), len = Math.sqrt(d[0] * d[0] + d[1] * d[1]);
  return len > 0 ? [d[0] / len, d[1] / len] : [1, 0];
}

function createPath(points, inTangents, outTangents, isClosed) {
  points = points || [];
  var zero = points.map(function () { return [0, 0]; });
  return new __Path(points, inTangents || zero, outTangents || zero, isClosed === undefined ? true : isClosed);
}

// ---------------------------------------------------------------------------------------------
// Object model.

function __mod(a, d) { return ((a % d) + d) % d; }

function __child(c, l, group, key) {
  var info = __h(__CHILD, c, l, group, key);
  if (info === null) return undefined;
  return info[1] ? __mkGroup(c, l, info) : __mkProp(c, l, info[0]);
}

// Comp ------------------------------------------------------------------------------------------

function Comp(c) { this.__c = c; this.__i = null; }
Comp.prototype = {
  __isComp: true,
  get __info() { return this.__i || (this.__i = __h(__COMP, this.__c)); },
  get name() { return this.__info[0]; },
  get width() { return this.__info[1]; },
  get height() { return this.__info[2]; },
  get duration() { return this.__info[3]; },
  get frameDuration() { return this.__info[4]; },
  get numLayers() { return this.__info[5]; },
  get pixelAspect() { return this.__info[6]; },
  get displayStartTime() { return this.__info[7]; },
  get bgColor() { return this.__info[8]; },
  get marker() { return new Markers(this.__c, -1); },
  // The topmost enabled camera active at the current time.
  get activeCamera() {
    for (var i = 1; i <= this.numLayers; i++) {
      var l = this.layer(i);
      if (l.__info[10] === 'Camera' && l.active) return l;
    }
    __err('Composition "' + this.name + '" has no active camera');
  },
  layer: function (a, rel) {
    if (a !== null && typeof a === 'object' && a.__isLayer) return this.layer(a.index + (rel || 0));
    var id = __h(__LAYER, this.__c, a);
    if (id === null) __err(typeof a === 'string' ? 'Layer named "' + a + '" does not exist' : 'Layer index ' + a + ' out of range');
    return new Layer(this.__c, id);
  },
  toString: function () { return '[object Comp]'; },
};

function comp(name) {
  var id = __h(__COMPNAME, String(name));
  if (id === null) __err('Comp named "' + name + '" does not exist');
  return new Comp(id);
}

// Footage (data-driven animation) and the project ------------------------------------------------

function Footage(info) { this.__i = info; this.__d = null; }
Footage.prototype = {
  __isFootage: true,
  get name() { return this.__i[0]; },
  get width() { return this.__i[1]; },
  get height() { return this.__i[2]; },
  get duration() { return this.__i[3]; },
  get frameDuration() { return this.__i[4]; },
  get pixelAspect() { return this.__i[5]; },
  get ntscDropFrame() { return false; },
  // The data file's text (JSON / CSV / TSV), or "" for media.
  get sourceText() { return this.__i[7] === null ? '' : this.__i[7]; },
  get __data() {
    if (this.__d !== null) return this.__d;
    var j = __h(__FDATA, this.__i[0]);
    if (j === null) __err('Footage "' + this.name + '" has no data (or it could not be parsed)');
    return (this.__d = JSON.parse(j));
  },
  // JSON: the parsed document; CSV/TSV: one object per row keyed by the header row.
  get sourceData() { return this.__data.data; },
  // CSV/TSV cell by [column, row] (0-based, rows after the header).
  dataValue: function (idx) {
    idx = __v(idx);
    var rows = this.__data.rows;
    if (rows === null) {
      // JSON: a path of keys / indexes.
      var v = this.__data.data;
      for (var i = 0; i < idx.length; i++) {
        if (v === null || v === undefined) __err('dataValue: no value at ' + JSON.stringify(idx));
        v = v[idx[i]];
      }
      return v;
    }
    var r = rows[idx[1]];
    if (r === undefined || r[idx[0]] === undefined) __err('dataValue: no cell at column ' + idx[0] + ', row ' + idx[1]);
    return r[idx[0]];
  },
  get dataKeyCount() { var r = this.__data.rows; return r === null ? 0 : r.length; },
  // The column names (CSV/TSV header row).
  get dataKeyNames() { return this.__data.header || []; },
  toString: function () { return '[object Footage]'; },
};

function footage(name) {
  var info = __h(__FOOT, String(name));
  if (info === null) __err('Footage named "' + name + '" does not exist');
  return new Footage(info);
}

var __projectInfo = null;
// The project's bits per channel (8, 16 or 32).
__global('colorDepth', function () { return thisProject.bitsPerChannel; });
var thisProject = {
  get __info() { return __projectInfo || (__projectInfo = __h(__PROJ)); },
  get fullPath() { return ''; },
  get bitsPerChannel() { return this.__info[0]; },
  get linearBlending() { return this.__info[1]; },
  toString: function () { return '[object Project]'; },
};

// Layer -----------------------------------------------------------------------------------------

function Layer(c, l) { this.__c = c; this.__l = l; this.__i = null; }

function __m3(m, p) {
  var x = p[0] || 0, y = p[1] || 0;
  var X = m[0] * x + m[1] * y + m[2], Y = m[3] * x + m[4] * y + m[5], W = m[6] * x + m[7] * y + m[8];
  if (W !== 0 && W !== 1) { X /= W; Y /= W; }
  return p.length > 2 ? [X, Y, 0] : [X, Y];
}
function __m3v(m, p) {
  var x = p[0] || 0, y = p[1] || 0;
  return p.length > 2 ? [m[0] * x + m[1] * y, m[3] * x + m[4] * y, 0] : [m[0] * x + m[1] * y, m[3] * x + m[4] * y];
}
function __m4(m, p, vec) {
  var x = p[0] || 0, y = p[1] || 0, z = p[2] || 0, k = vec ? 0 : 1;
  var X = m[0] * x + m[1] * y + m[2] * z + m[3] * k;
  var Y = m[4] * x + m[5] * y + m[6] * z + m[7] * k;
  var Z = m[8] * x + m[9] * y + m[10] * z + m[11] * k;
  var W = vec ? 1 : m[12] * x + m[13] * y + m[14] * z + m[15];
  if (W !== 0 && W !== 1) { X /= W; Y /= W; Z /= W; }
  return [X, Y, Z];
}

Layer.prototype = {
  __isLayer: true,
  get __info() { return this.__i || (this.__i = __h(__LINFO, this.__c, this.__l)); },
  get name() { return this.__info[0]; },
  get index() { return this.__info[1]; },
  get inPoint() { return this.__info[2]; },
  get outPoint() { return this.__info[3]; },
  get startTime() { return this.__info[4]; },
  get width() { return this.__info[5]; },
  get height() { return this.__info[6]; },
  get hasParent() { return this.__info[7] !== null; },
  get parent() {
    var p = this.__info[7];
    if (p === null) __err('Layer "' + this.name + '" has no parent');
    return new Layer(this.__c, p);
  },
  get hasVideo() { return this.__info[8]; },
  get enabled() { return this.__info[11]; },
  get active() { return this.__info[11] && time >= this.inPoint && time < this.outPoint; },
  get containingComp() { return new Comp(this.__c); },
  // The source item (AE's Layer.source): a Comp for a precomp layer, a Footage for footage, solid
  // and model layers, null for text, shape, null, camera and light layers (#809).
  get source() {
    var s = __h(__SOURCE, this.__c, this.__l);
    if (s === null) return null;
    if (s[0] === 'comp') return new Comp(s[1]);
    return new Footage([s[2], s[3], s[4], s[5], s[6], s[7], s[0], null, null]);
  },
  __group: function (k) {
    var g = __child(this.__c, this.__l, '', k);
    if (g === undefined) __err('Layer "' + this.name + '" has no property "' + k + '"');
    return g;
  },
  get transform() { return this.__group('transform'); },
  get position() { return this.transform.position; },
  get scale() { return this.transform.scale; },
  get rotation() { return this.transform.rotation; },
  get opacity() { return this.transform.opacity; },
  get anchorPoint() { return this.transform.anchorPoint; },
  get text() { return this.__group('text'); },
  get cameraOption() { return this.__group('cameraOptions'); },
  get lightOption() { return this.__group('lightOptions'); },
  get marker() { return new Markers(this.__c, this.__l); },
  effect: function (k) {
    var g = __child(this.__c, this.__l, '', 'effects');
    var e = g === undefined ? undefined : __child(g.__c, g.__l, g.__p, k);
    if (e === undefined) __err('Effect ' + (typeof k === 'string' ? '"' + k + '"' : k) + ' not found on layer "' + this.name + '"');
    return e;
  },
  mask: function (k) { return this.__group('masks').__get(k); },
  content: function (k) { return this.__group('contents').__get(k); },
  __x: function (t) { return __h(__XFORM, this.__c, this.__l, t === undefined ? time : t); },
  toComp: function (p, t) { return __m3(this.__x(t)[0], __v(p)); },
  fromComp: function (p, t) { return __m3(this.__x(t)[1], __v(p)); },
  toWorld: function (p, t) { return __m4(this.__x(t)[2], __v(p), false); },
  fromWorld: function (p, t) { return __m4(this.__x(t)[3], __v(p), false); },
  toCompVec: function (p, t) { return __m3v(this.__x(t)[0], __v(p)); },
  fromCompVec: function (p, t) { return __m3v(this.__x(t)[1], __v(p)); },
  toWorldVec: function (p, t) { return __m4(this.__x(t)[2], __v(p), true); },
  fromWorldVec: function (p, t) { return __m4(this.__x(t)[3], __v(p), true); },
  sourceRectAtTime: function (t, includeExtents) {
    var r = __h(__RECT, this.__c, this.__l, t === undefined ? time : t, !!includeExtents);
    return { top: r[0], left: r[1], width: r[2], height: r[3] };
  },
  // Average straight RGBA of the layer's pixels in `point ± radius` (layer space).
  sampleImage: function (point, radius, postEffect, t) {
    point = __v(point);
    radius = radius === undefined ? [0.5, 0.5] : __v(radius);
    if (!Array.isArray(radius)) radius = [radius, radius];
    return __h(__SAMPLE, this.__c, this.__l, point[0], point[1], radius[0], radius.length > 1 ? radius[1] : radius[0],
      postEffect === undefined ? true : !!__v(postEffect), t === undefined ? time : __v(t));
  },
  toString: function () { return '[object Layer]'; },
};

// Property groups (callable: `effect("Slider Control")("Slider")`) ---------------------------------

var __GroupProto = {
  __isGroup: true,
  __get: function (k) {
    var r = __child(this.__c, this.__l, this.__p, k);
    if (r === undefined) __err('Property ' + (typeof k === 'string' ? '"' + k + '"' : k) + ' not found in "' + this.__info[2] + '"');
    return r;
  },
  param: function (k) { return this.__get(k); },
  property: function (k) { return this.__get(k); },
  content: function (k) {
    var inner = __child(this.__c, this.__l, this.__p, 'contents');
    return (inner !== undefined && inner.__isGroup ? inner : this).__get(k);
  },
  get name() { return this.__info[2]; },
  get matchName() { return this.__info[3]; },
  get numProperties() { return this.__info[4]; },
  get active() { return true; },
  toString: function () { return '[object PropertyGroup]'; },
};

var __groupHandler = {
  get: function (t, k) {
    if (typeof k !== 'string' || k in t) return t[k];
    return __child(t.__c, t.__l, t.__p, k);
  },
};

function __mkGroup(c, l, info) {
  var f = function (k) { return f.__get(k); };
  delete f.name;
  delete f.length;
  Object.setPrototypeOf(f, __GroupProto);
  f.__c = c; f.__l = l; f.__p = info[0]; f.__info = info;
  return new Proxy(f, __groupHandler);
}

// Properties ------------------------------------------------------------------------------------

function Prop(c, l, path) {
  this.__c = c; this.__l = l; this.__p = path; this.__i = null;
  this.__self = __S !== null && c === __S.c && l === __S.l && path === __S.path;
}

function __avg(vals) {
  var s = vals[0];
  for (var i = 1; i < vals.length; i++) s = add(s, vals[i]);
  return div(s, vals.length);
}

function __loop(p, out, type, n, dur) {
  type = type === undefined ? 'cycle' : String(type).toLowerCase();
  var times = p.__info[4], N = times.length, t = time;
  if (N === 0) return p.value;
  var tStart, tEnd, h = 1e-3, k, ph;
  if (out) {
    tEnd = times[N - 1];
    if (t <= tEnd) return p.value;
    if (dur !== undefined && dur > 0) tStart = tEnd - dur;
    else if (n > 0 && N - 1 - n >= 0) tStart = times[N - 1 - n];
    else tStart = times[0];
  } else {
    tStart = times[0];
    if (t >= tStart) return p.value;
    if (dur !== undefined && dur > 0) tEnd = tStart + dur;
    else if (n > 0 && n < N) tEnd = times[n];
    else tEnd = times[N - 1];
  }
  var d = tEnd - tStart;
  if (type === 'continue') {
    if (out) {
      var ve = p.valueAtTime(tEnd);
      return add(ve, mul(div(sub(ve, p.valueAtTime(tEnd - h)), h), t - tEnd));
    }
    var vs = p.valueAtTime(tStart);
    return add(vs, mul(div(sub(p.valueAtTime(tStart + h), vs), h), t - tStart));
  }
  if (!(d > 0)) return p.value;
  ph = __mod(t - tStart, d);
  if (type === 'pingpong') {
    if (out) {
      k = Math.floor((t - tStart) / d);
      return p.valueAtTime(k % 2 === 1 ? tEnd - ph : tStart + ph);
    }
    k = Math.floor((tEnd - t) / d);
    return p.valueAtTime(k % 2 === 1 ? tStart + __mod(tStart - t, d) : tEnd - __mod(tStart - t, d));
  }
  if (type === 'offset') {
    k = Math.floor((t - tStart) / d);
    return add(p.valueAtTime(tStart + ph), mul(sub(p.valueAtTime(tEnd), p.valueAtTime(tStart)), k));
  }
  return p.valueAtTime(tStart + ph);
}

Prop.prototype = {
  __isProp: true,
  get __info() { return this.__i || (this.__i = __h(__PINFO, this.__c, this.__l, this.__p)); },
  get name() { return this.__info[0]; },
  get __uid() { return this.__info[1]; },
  get numKeys() { return this.__info[4].length; },
  get propertyIndex() { return this.__info[7]; },
  get matchName() { return this.__info[8]; },
  __wrap: function (v) {
    var kind = this.__info[2];
    if (kind === 'layer') return v === null ? null : new Layer(this.__c, v);
    if (kind === 'path' && v !== null) return new __Path(v[0], v[1], v[2], v[3]);
    return v;
  },
  get value() { return this.__self ? value : this.valueAtTime(time); },
  valueAtTime: function (t) {
    t = __v(t);
    if (this.__self && t === time) return value;
    return this.__wrap(__h(__VALUE, this.__c, this.__l, this.__p, t, this.__self));
  },
  velocityAtTime: function (t) {
    var h = 1e-3;
    return div(sub(this.valueAtTime(t + h), this.valueAtTime(t - h)), 2 * h);
  },
  get velocity() { return this.velocityAtTime(time); },
  speedAtTime: function (t) { return length(this.velocityAtTime(t)); },
  get speed() { return this.speedAtTime(time); },
  key: function (n) {
    var info = this.__info, N = info[4].length;
    if (!(n >= 1 && n <= N)) __err('Key index ' + n + ' out of range (property has ' + N + ' keys)');
    n = Math.floor(n);
    return { time: info[4][n - 1], value: this.__wrap(info[5][n - 1]), index: n };
  },
  nearestKey: function (t) {
    var times = this.__info[4];
    if (times.length === 0) __err('Property has no keyframes');
    var best = 0;
    for (var i = 1; i < times.length; i++) if (Math.abs(times[i] - t) < Math.abs(times[best] - t)) best = i;
    return this.key(best + 1);
  },
  __seedIndex: function () { return this.__self ? __S.idx : new Layer(this.__c, this.__l).index; },
  wiggle: function (freq, amp, octaves, ampMult, t) {
    if (octaves === undefined) octaves = 1;
    if (ampMult === undefined) ampMult = 0.5;
    if (t === undefined) t = time;
    var base = this.valueAtTime(t);
    var dims = Array.isArray(base) ? base.length : 1;
    var off = __wig(this.__seedIndex(), this.__uid, dims, __v(freq), __v(amp), octaves, ampMult, t);
    if (!Array.isArray(base)) return base + off[0];
    var r = base.slice();
    for (var i = 0; i < r.length; i++) r[i] += off[i];
    return r;
  },
  temporalWiggle: function (freq, amp, octaves, ampMult, t) {
    if (octaves === undefined) octaves = 1;
    if (ampMult === undefined) ampMult = 0.5;
    if (t === undefined) t = time;
    var off = __wig(this.__seedIndex(), this.__uid, 1, __v(freq), __v(amp), octaves, ampMult, t);
    return this.valueAtTime(t + off[0]);
  },
  smooth: function (width, samples, t) {
    if (width === undefined) width = 0.2;
    if (samples === undefined) samples = 5;
    if (t === undefined) t = time;
    samples = Math.max(1, Math.round(samples));
    if (samples === 1) return this.valueAtTime(t);
    var vals = [];
    for (var i = 0; i < samples; i++) vals.push(this.valueAtTime(t - width / 2 + width * i / (samples - 1)));
    return __avg(vals);
  },
  loopOut: function (type, n) { return __loop(this, true, type, n || 0); },
  loopIn: function (type, n) { return __loop(this, false, type, n || 0); },
  loopOutDuration: function (type, d) { return __loop(this, true, type, 0, d === undefined ? 0 : d); },
  loopInDuration: function (type, d) { return __loop(this, false, type, 0, d === undefined ? 0 : d); },
  points: function (t) { return this.valueAtTime(t === undefined ? time : t).points(); },
  inTangents: function (t) { return this.valueAtTime(t === undefined ? time : t).inTangents(); },
  outTangents: function (t) { return this.valueAtTime(t === undefined ? time : t).outTangents(); },
  isClosed: function () { return this.value.isClosed(); },
  pointOnPath: function (pct, t) { return this.valueAtTime(t === undefined ? time : t).pointOnPath(pct); },
  tangentOnPath: function (pct, t) { return this.valueAtTime(t === undefined ? time : t).tangentOnPath(pct); },
  normalOnPath: function (pct, t) { return this.valueAtTime(t === undefined ? time : t).normalOnPath(pct); },
  // Source Text style API.
  __docAt: function (t) {
    var d = __h(__DOC, this.__c, this.__l, this.__p, t, this.__self);
    if (d === null) __err('Property "' + this.name + '" is not a Source Text property');
    return JSON.parse(d);
  },
  get style() { return this.getStyleAt(0); },
  getStyleAt: function (i, t) {
    if (t === undefined) t = time;
    return new __TextStyle(this.__docAt(__v(t)), i === undefined ? 0 : Math.floor(__v(i)), [], false);
  },
  createStyle: function () { return new __TextStyle(this.__docAt(time), 0, [], true); },
  valueOf: function () { return this.value; },
  toString: function () { return String(this.value); },
};

var __propHandler = {
  get: function (t, k) {
    if (typeof k !== 'string') return t[k];
    // A layer-valued property (a Layer Control's Layer) is named as its layer (#809).
    if (k === 'name' && t.__info[2] === 'layer') {
      var lv = t.value;
      if (lv !== null && lv !== undefined && lv.__isLayer) return lv.name;
    }
    if (k in t) return t[k];
    var v = t.value;
    if (v === null || v === undefined) return undefined;
    var x = v[k];
    // Property groups are callable proxies: hand them over as they are, not bound like methods (#809).
    if (x !== null && x !== undefined && x.__isGroup) return x;
    return typeof x === 'function' ? x.bind(v) : x;
  },
};

function __mkProp(c, l, path) {
  return new Proxy(new Prop(c, l, path), __propHandler);
}

// Text styles (the Source Text style API) ------------------------------------------------------
// A TextStyle is the source document (as JSON from the host), the character index it reads its
// attributes at, and the setter calls made on it ([key, value, start, count]). Setters return
// new objects; returning a style from a Source Text expression applies it.

function __TextStyle(doc, idx, ops, empty) { this.__d = doc; this.__i = idx; this.__ops = ops; this.__empty = empty; }

function __styleSetter(key, conv) {
  return function (v, start, count) {
    v = __v(v);
    if (conv) v = conv(v, this);
    var s = start === undefined ? -1 : Math.max(0, Math.floor(__v(start)));
    var n = count === undefined ? -1 : Math.max(0, Math.floor(__v(count)));
    return new __TextStyle(this.__d, this.__i, this.__ops.concat([[key, v, s, n]]), this.__empty);
  };
}

function __colorIn(c) {
  if (typeof c === 'string') return c;
  return [c[0], c[1], c[2]];
}

__TextStyle.prototype = {
  __isTextStyle: true,
  __base: function () {
    var runs = this.__d.runs, pos = 0, i = this.__i;
    for (var k = 0; k < runs.length; k++) {
      if (i < pos + runs[k].len || k === runs.length - 1) return runs[k].style;
      pos += runs[k].len;
    }
    return {};
  },
  __para: function () {
    var t = this.text, p = 0;
    for (var k = 0; k < Math.min(this.__i, t.length); k++) if (t[k] === '\n' || t[k] === '\r') p++;
    var paras = this.__d.paras;
    return paras[Math.min(p, paras.length - 1)];
  },
  __get: function (key, para) {
    for (var k = this.__ops.length - 1; k >= 0; k--) {
      var o = this.__ops[k];
      if (o[0] !== key) continue;
      if (o[2] < 0 || (this.__i >= o[2] && (o[3] < 0 || this.__i < o[2] + o[3]))) return o[1];
    }
    return (para ? this.__para() : this.__base())[key];
  },
  get text() {
    var t = this.__d.text;
    for (var k = 0; k < this.__ops.length; k++) {
      var o = this.__ops[k];
      if (o[0] !== 'text') continue;
      if (o[2] < 0) t = String(o[1]);
      else t = t.slice(0, o[2]) + String(o[1]) + (o[3] < 0 ? '' : t.slice(o[2] + o[3]));
    }
    return t;
  },
  get font() { return this.__get('font'); },
  get fontSize() { return this.__get('size'); },
  get isFauxBold() { return this.__get('fauxBold'); },
  get isFauxItalic() { return this.__get('fauxItalic'); },
  get isAllCaps() { return this.__get('allCaps'); },
  get isSmallCaps() { return this.__get('smallCaps'); },
  get tracking() { return this.__get('tracking'); },
  get autoLeading() { return this.__get('leading') === 'auto'; },
  get leading() { var l = this.__get('leading'); return l === 'auto' ? this.fontSize * 1.2 : l; },
  get baselineShift() { return this.__get('baselineShift'); },
  get applyFill() { return this.__get('applyFill'); },
  get fillColor() { var c = this.__get('fill'); return [c[0], c[1], c[2]]; },
  get applyStroke() { return this.__get('applyStroke'); },
  get strokeColor() { var c = this.__get('stroke'); return [c[0], c[1], c[2]]; },
  get strokeWidth() { return this.__get('strokeWidth'); },
  get horizontalScaling() { return this.__get('hScale'); },
  get verticalScaling() { return this.__get('vScale'); },
  get tsume() { return this.__get('tsume'); },
  get baselineOption() { return this.__get('baseline'); },
  get isSuperscript() { return this.__get('baseline') === 'superscript'; },
  get isSubscript() { return this.__get('baseline') === 'subscript'; },
  get kerningType() { var k = this.__get('kerning'); return typeof k === 'number' ? 'manual' : k; },
  get kerning() { var k = this.__get('kerning'); return typeof k === 'number' ? k : 0; },
  get isLigature() { return this.__get('ligatures'); },
  get justification() { return this.__get('justify', true); },
  get firstLineIndent() { return this.__get('indentFirst', true); },
  get startIndent() { return this.__get('indentLeft', true); },
  get endIndent() { return this.__get('indentRight', true); },
  get spaceBefore() { return this.__get('spaceBefore', true); },
  get spaceAfter() { return this.__get('spaceAfter', true); },
  get direction() { return this.__get('direction', true); },
  get isEveryLineComposer() { return this.__get('composer', true) === 'everyLine'; },
  get isHangingRoman() { return this.__get('hangingPunctuation', true); },
  setText: __styleSetter('text', function (v) { return String(v); }),
  replaceText: __styleSetter('text', function (v) { return String(v); }),
  setFont: __styleSetter('font', function (v) { return String(v); }),
  setFontSize: __styleSetter('size'),
  setFauxBold: __styleSetter('fauxBold', Boolean),
  setFauxItalic: __styleSetter('fauxItalic', Boolean),
  setAllCaps: __styleSetter('allCaps', Boolean),
  setSmallCaps: __styleSetter('smallCaps', Boolean),
  setTracking: __styleSetter('tracking'),
  setLeading: __styleSetter('leading'),
  setAutoLeading: __styleSetter('leading', function (v, s) { return v ? 'auto' : s.leading; }),
  setBaselineShift: __styleSetter('baselineShift'),
  setApplyFill: __styleSetter('applyFill', Boolean),
  setFillColor: __styleSetter('fill', __colorIn),
  setApplyStroke: __styleSetter('applyStroke', Boolean),
  setStrokeColor: __styleSetter('stroke', __colorIn),
  setStrokeWidth: __styleSetter('strokeWidth'),
  setHorizontalScaling: __styleSetter('hScale'),
  setVerticalScaling: __styleSetter('vScale'),
  setTsume: __styleSetter('tsume'),
  setBaselineOption: __styleSetter('baseline', function (v) { return String(v).toLowerCase().replace('_baseline', ''); }),
  // Canonical baseline operations also make the returned TextStyle's getters reflect
  // the pending edit, rather than waiting for the host to apply a superscript flag.
  setSuperscript: __styleSetter('baseline', function (v) { return v ? 'superscript' : 'normal'; }),
  setSubscript: __styleSetter('baseline', function (v) { return v ? 'subscript' : 'normal'; }),
  setKerningType: __styleSetter('kerning', function (v) { v = String(v).toLowerCase(); return v === 'manual' ? 0 : v; }),
  setKerning: __styleSetter('kerning', Number),
  setLigature: __styleSetter('ligatures', Boolean),
  setJustification: __styleSetter('justify', function (v) { return String(v); }),
  setFirstLineIndent: __styleSetter('indentFirst'),
  setStartIndent: __styleSetter('indentLeft'),
  setEndIndent: __styleSetter('indentRight'),
  setSpaceBefore: __styleSetter('spaceBefore'),
  setSpaceAfter: __styleSetter('spaceAfter'),
  setDirection: __styleSetter('direction', function (v) { v = String(v).toLowerCase(); return v.indexOf('right') === 0 || v === 'rtl' ? 'rtl' : 'ltr'; }),
  setEveryLineComposer: __styleSetter('composer', function (v) { return v ? 'everyLine' : 'singleLine'; }),
  setHangingRoman: __styleSetter('hangingPunctuation', Boolean),
  __out: function () {
    return { __style: true, __json: JSON.stringify({ doc: this.__empty ? null : this.__d.doc, ops: this.__ops }) };
  },
  toString: function () { return this.text; },
  valueOf: function () { return this.text; },
};
Object.defineProperty(__TextStyle.prototype, 'length', { get: function () { return this.text.length; } });

// Markers ---------------------------------------------------------------------------------------

function Markers(c, l) { this.__c = c; this.__l = l; this.__m = null; }
Markers.prototype = {
  get __list() { return this.__m || (this.__m = __h(__MARKERS, this.__c, this.__l)); },
  get numKeys() { return this.__list.length; },
  key: function (n) {
    var list = this.__list, i;
    if (typeof n === 'string') {
      for (i = 0; i < list.length; i++) if (list[i][2] === n) return this.key(i + 1);
      __err('Marker "' + n + '" not found');
    }
    if (!(n >= 1 && n <= list.length)) __err('Marker index ' + n + ' out of range');
    var m = list[n - 1];
    var params = {};
    for (var k = 0; k < m[8].length; k++) params[m[8][k][0]] = m[8][k][1];
    return {
      time: m[0], duration: m[1], comment: m[2], chapter: m[3], url: m[4], index: n,
      frameTarget: m[5], eventCuePoint: m[6], cuePointName: m[7], parameters: params, protectedRegion: m[9],
    };
  },
  nearestKey: function (t) {
    var list = this.__list;
    if (list.length === 0) __err('No markers');
    var best = 0;
    for (var i = 1; i < list.length; i++) if (Math.abs(list[i][0] - t) < Math.abs(list[best][0] - t)) best = i;
    return this.key(best + 1);
  },
};

// ---------------------------------------------------------------------------------------------
// Unqualified names: thisLayer / thisProperty attributes and the `this*` objects. They're
// accessors on the global object; assigning one (`var width = 10`) shadows it for the current
// evaluation only.

function __global(name, get) {
  Object.defineProperty(globalThis, name, {
    configurable: true,
    get: function () { return name in __S.vars ? __S.vars[name] : get(); },
    set: function (v) { __S.vars[name] = v; },
  });
}

__global('thisComp', function () { return __S.comp || (__S.comp = new Comp(__S.c)); });
__global('thisLayer', function () { return __S.layer || (__S.layer = new Layer(__S.c, __S.l)); });
__global('thisProperty', function () { return __S.prop || (__S.prop = __mkProp(__S.c, __S.l, __S.path)); });
['name', 'index', 'inPoint', 'outPoint', 'startTime', 'width', 'height', 'hasParent', 'parent', 'hasVideo', 'enabled', 'active',
  'transform', 'position', 'scale', 'rotation', 'opacity', 'anchorPoint', 'text', 'marker'].forEach(function (k) {
  __global(k, function () { return thisLayer[k]; });
});
['numKeys', 'velocity', 'speed'].forEach(function (k) {
  __global(k, function () { return thisProperty[k]; });
});

function effect(k) { return thisLayer.effect(k); }
function mask(k) { return thisLayer.mask(k); }
function content(k) { return thisLayer.content(k); }
function toComp(p, t) { return thisLayer.toComp(p, t); }
function fromComp(p, t) { return thisLayer.fromComp(p, t); }
function toWorld(p, t) { return thisLayer.toWorld(p, t); }
function fromWorld(p, t) { return thisLayer.fromWorld(p, t); }
function toCompVec(p, t) { return thisLayer.toCompVec(p, t); }
function fromCompVec(p, t) { return thisLayer.fromCompVec(p, t); }
function toWorldVec(p, t) { return thisLayer.toWorldVec(p, t); }
function fromWorldVec(p, t) { return thisLayer.fromWorldVec(p, t); }
function sourceRectAtTime(t, e) { return thisLayer.sourceRectAtTime(t, e); }

function wiggle(f, a, o, m, t) { return thisProperty.wiggle(f, a, o, m, t); }
function temporalWiggle(f, a, o, m, t) { return thisProperty.temporalWiggle(f, a, o, m, t); }
function smooth(w, s, t) { return thisProperty.smooth(w, s, t); }
function loopOut(type, n) { return thisProperty.loopOut(type, n); }
function loopIn(type, n) { return thisProperty.loopIn(type, n); }
function loopOutDuration(type, d) { return thisProperty.loopOutDuration(type, d); }
function loopInDuration(type, d) { return thisProperty.loopInDuration(type, d); }
function valueAtTime(t) { return thisProperty.valueAtTime(t); }
function velocityAtTime(t) { return thisProperty.velocityAtTime(t); }
function speedAtTime(t) { return thisProperty.speedAtTime(t); }
function key(n) { return thisProperty.key(n); }
function nearestKey(t) { return thisProperty.nearestKey(t); }
