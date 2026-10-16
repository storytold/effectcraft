// EffectCraft Transfer: serialises the Figma selection into the node JSON EffectCraft's
// `file.importFigma` reads (the shape of Figma's REST API `nodes` reply, plus a few plug-in-only
// fields such as `absoluteTransform`, `vectorPaths` and `pointCount`), renders what has no
// native counterpart to PNG (2x), and hands it to the panel (ui.html), which posts it to
// `effectcraft-cli figma-bridge`. Written against Figma's public Plugin API documentation.

figma.showUI(__html__, { width: 300, height: 236, themeColors: true });

var MIXED = figma.mixed;
var NATIVE_TYPES = {
  FRAME: 1, GROUP: 1, COMPONENT: 1, COMPONENT_SET: 1, INSTANCE: 1, SECTION: 1, BOOLEAN_OPERATION: 1,
  RECTANGLE: 1, ELLIPSE: 1, LINE: 1, VECTOR: 1, STAR: 1, POLYGON: 1, TEXT: 1, SLICE: 1
};
var IMAGE_PAINTS = { IMAGE: 1, VIDEO: 1, EMOJI: 1, PATTERN: 1 };
var MAX_NODES = 20000;

function isMixed(v) { return v === MIXED || typeof v === 'symbol'; }

function rgba(c, a) {
  return { r: c.r, g: c.g, b: c.b, a: (c.a !== undefined ? c.a : (a !== undefined ? a : 1)) };
}

// Plug-in gradients carry `gradientTransform` (node's normalised space → gradient space);
// the REST format carries handle positions in the node's normalised space.
function invert(m) {
  var a = m[0][0], c = m[0][1], e = m[0][2], b = m[1][0], d = m[1][1], f = m[1][2];
  var det = a * d - b * c;
  if (Math.abs(det) < 1e-12) return [[1, 0, 0], [0, 1, 0]];
  return [[d / det, -c / det, (c * f - d * e) / det], [-b / det, a / det, (b * e - a * f) / det]];
}
function apply(m, x, y) { return { x: m[0][0] * x + m[0][1] * y + m[0][2], y: m[1][0] * x + m[1][1] * y + m[1][2] }; }

function paint(p) {
  var out = { type: p.type, visible: p.visible !== false, opacity: p.opacity !== undefined ? p.opacity : 1, blendMode: p.blendMode || 'NORMAL' };
  if (p.type === 'SOLID') {
    out.color = rgba(p.color, 1);
  } else if (p.type.indexOf('GRADIENT_') === 0) {
    var inv = invert(p.gradientTransform);
    var linear = p.type === 'GRADIENT_LINEAR';
    out.gradientHandlePositions = linear
      ? [apply(inv, 0, 0.5), apply(inv, 1, 0.5), apply(inv, 0, 1)]
      : [apply(inv, 0.5, 0.5), apply(inv, 1, 0.5), apply(inv, 0.5, 1)];
    out.gradientStops = p.gradientStops.map(function (s) { return { position: s.position, color: rgba(s.color) }; });
  } else if (p.type === 'IMAGE') {
    out.scaleMode = p.scaleMode;
  }
  return out;
}

function paints(list) {
  if (!list || isMixed(list)) return [];
  return list.map(paint);
}

function hasImagePaint(n) {
  var lists = [n.fills, n.strokes];
  for (var i = 0; i < lists.length; i++) {
    var l = lists[i];
    if (!l || isMixed(l)) continue;
    for (var j = 0; j < l.length; j++) if (l[j].visible !== false && IMAGE_PAINTS[l[j].type]) return true;
  }
  return false;
}

function needsRaster(n) {
  if (!NATIVE_TYPES[n.type]) return n.type + ' node';
  if (hasImagePaint(n)) return 'image fill';
  return null;
}

function bounds(b) { return b ? { x: b.x, y: b.y, width: b.width, height: b.height } : undefined; }

function effects(n) {
  if (!n.effects || isMixed(n.effects)) return [];
  return n.effects.map(function (e) {
    var o = { type: e.type, visible: e.visible !== false, radius: e.radius };
    if (e.color) o.color = rgba(e.color);
    if (e.offset) o.offset = { x: e.offset.x, y: e.offset.y };
    if (e.spread !== undefined) o.spread = e.spread;
    if (e.blendMode) o.blendMode = e.blendMode;
    if (e.showShadowBehindNode !== undefined) o.showShadowBehindNode = e.showShadowBehindNode;
    return o;
  });
}

function lineHeightPx(lh, size) {
  if (!lh || isMixed(lh) || lh.unit === 'AUTO') return size * 1.21;
  if (lh.unit === 'PERCENT') return lh.value / 100 * size;
  return lh.value;
}
function letterSpacingPx(ls, size) {
  if (!ls || isMixed(ls)) return 0;
  return ls.unit === 'PERCENT' ? ls.value / 100 * size : ls.value;
}

function typeStyle(seg) {
  var s = {
    fontFamily: seg.fontName.family,
    fontStyle: seg.fontName.style,
    fontSize: seg.fontSize,
    letterSpacing: letterSpacingPx(seg.letterSpacing, seg.fontSize),
    lineHeightPx: lineHeightPx(seg.lineHeight, seg.fontSize),
    textCase: seg.textCase,
    textDecoration: seg.textDecoration
  };
  if (seg.fills) s.fills = paints(seg.fills);
  return s;
}

function text(n, out) {
  out.characters = n.characters;
  var segs = n.getStyledTextSegments(['fontName', 'fontSize', 'fills', 'letterSpacing', 'lineHeight', 'textCase', 'textDecoration']);
  var base = segs.length ? typeStyle(segs[0]) : {};
  base.textAlignHorizontal = n.textAlignHorizontal;
  base.textAlignVertical = n.textAlignVertical;
  base.textAutoResize = n.textAutoResize;
  if (!isMixed(n.paragraphSpacing)) base.paragraphSpacing = n.paragraphSpacing;
  out.style = base;
  out.textAutoResize = n.textAutoResize;
  if (segs.length) out.fills = paints(segs[0].fills);
  // Runs: one style id per UTF-16 code unit (JS string indices are UTF-16 already).
  var overrides = [];
  var table = {};
  var key0 = JSON.stringify(typeStyle(segs[0] || { fontName: {}, fontSize: 12 }));
  var ids = {};
  var next = 1;
  for (var i = 0; i < segs.length; i++) {
    var st = typeStyle(segs[i]);
    var key = JSON.stringify(st);
    var id = 0;
    if (key !== key0) {
      if (!ids[key]) { ids[key] = next++; table[String(ids[key])] = st; }
      id = ids[key];
    }
    for (var k = segs[i].start; k < segs[i].end; k++) overrides[k] = id;
  }
  if (next > 1) { out.characterStyleOverrides = overrides; out.styleOverrideTable = table; }
}

function serialize(n, rasters, count) {
  count.n++;
  if (count.n > MAX_NODES) return null;
  var out = {
    id: n.id, name: n.name, type: n.type === 'POLYGON' ? 'REGULAR_POLYGON' : n.type, visible: n.visible,
    opacity: n.opacity, blendMode: n.blendMode,
    absoluteTransform: n.absoluteTransform,
    size: { x: n.width, y: n.height },
    absoluteBoundingBox: bounds(n.absoluteBoundingBox),
    absoluteRenderBounds: bounds(n.absoluteRenderBounds)
  };
  if (n.isMask) { out.isMask = true; out.maskType = n.maskType || 'ALPHA'; }
  if ('effects' in n) out.effects = effects(n);
  var raster = needsRaster(n);
  if (raster) {
    rasters.push({ node: n, reason: raster });
    return out;
  }
  if ('fills' in n) out.fills = paints(n.fills);
  if ('strokes' in n) {
    out.strokes = paints(n.strokes);
    var w = n.strokeWeight;
    if (isMixed(w)) w = Math.max(n.strokeTopWeight || 0, n.strokeRightWeight || 0, n.strokeBottomWeight || 0, n.strokeLeftWeight || 0);
    out.strokeWeight = w;
    out.strokeAlign = n.strokeAlign;
    out.strokeCap = isMixed(n.strokeCap) ? 'NONE' : n.strokeCap;
    out.strokeJoin = isMixed(n.strokeJoin) ? 'MITER' : n.strokeJoin;
    if (n.strokeMiterLimit) out.strokeMiterLimit = n.strokeMiterLimit;
    if (n.dashPattern && n.dashPattern.length) out.strokeDashes = n.dashPattern.slice();
  }
  if ('clipsContent' in n) out.clipsContent = n.clipsContent;
  if ('cornerRadius' in n) {
    if (isMixed(n.cornerRadius)) {
      out.rectangleCornerRadii = [n.topLeftRadius, n.topRightRadius, n.bottomRightRadius, n.bottomLeftRadius];
    } else {
      out.cornerRadius = n.cornerRadius;
    }
  }
  if (n.type === 'ELLIPSE') out.arcData = n.arcData;
  if (n.type === 'STAR' || n.type === 'POLYGON') {
    out.pointCount = n.pointCount;
    if (n.type === 'STAR') out.innerRadius = n.innerRadius;
  }
  if (n.type === 'VECTOR') {
    try { out.vectorPaths = n.vectorPaths.map(function (p) { return { path: p.data, windingRule: p.windingRule }; }); } catch (e) { /* fall back to fillGeometry */ }
  }
  if (n.type === 'VECTOR' || n.type === 'STAR' || n.type === 'POLYGON' || n.type === 'BOOLEAN_OPERATION' || n.type === 'ELLIPSE') {
    try { out.fillGeometry = n.fillGeometry.map(function (g) { return { path: g.data, windingRule: g.windingRule }; }); } catch (e) { /* none */ }
  }
  if (n.type === 'TEXT') text(n, out);
  if ('children' in n && n.type !== 'BOOLEAN_OPERATION') {
    out.children = [];
    for (var i = 0; i < n.children.length; i++) {
      var c = serialize(n.children[i], rasters, count);
      if (c) out.children.push(c);
    }
  }
  return out;
}

async function collect() {
  var sel = figma.currentPage.selection;
  if (!sel.length) throw new Error('Select something to send.');
  var rasters = [];
  var count = { n: 0 };
  var nodes = sel.map(function (n) { return serialize(n, rasters, count); }).filter(Boolean);
  var images = {};
  for (var i = 0; i < rasters.length; i++) {
    var r = rasters[i];
    figma.ui.postMessage({ type: 'progress', text: 'Rendering ' + (i + 1) + ' of ' + rasters.length + '…' });
    try {
      var bytes = await r.node.exportAsync({ format: 'PNG', constraint: { type: 'SCALE', value: 2 }, useAbsoluteBounds: true });
      images[r.node.id] = { png: figma.base64Encode(bytes) };
    } catch (e) {
      // EffectCraft notes the missing image.
    }
  }
  return { source: 'figma-plugin', fileKey: figma.fileKey || null, nodes: nodes, images: images };
}

figma.ui.onmessage = async function (msg) {
  if (msg.type === 'collect') {
    try {
      var payload = await collect();
      figma.ui.postMessage({ type: 'payload', payload: payload, target: msg.target });
    } catch (e) {
      figma.ui.postMessage({ type: 'error', text: String(e && e.message ? e.message : e) });
    }
  } else if (msg.type === 'save-code') {
    await figma.clientStorage.setAsync('pairingCode', msg.code || '');
  } else if (msg.type === 'save-target') {
    await figma.clientStorage.setAsync('target', msg.target || 'newComp');
  } else if (msg.type === 'notify') {
    figma.notify(msg.text);
  }
};

(async function () {
  var code = await figma.clientStorage.getAsync('pairingCode');
  var target = await figma.clientStorage.getAsync('target');
  figma.ui.postMessage({ type: 'init', code: code || '', target: target || 'newComp', selection: figma.currentPage.selection.length });
})();

figma.on('selectionchange', function () {
  figma.ui.postMessage({ type: 'selection', count: figma.currentPage.selection.length });
});
