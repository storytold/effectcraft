//! SVG import (see README.md for the supported subset and the specifications used).
//!
//! [`parse`] resolves styles, `use` references and gradients into a small render tree
//! ([`Doc`]): groups with transforms and opacity, and shapes with fills and strokes in their own
//! user space. [`rasterize`] draws it at any scale; the engine converts it to shape layers.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod color;
mod pathdata;
mod raster;

use std::collections::HashMap;

pub use color::parse_color;
pub use kurbo::{Affine, BezPath};
pub use pathdata::parse_path_data;
pub use raster::{blend_pixel, rasterize, rasterize_with};

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    Xml(String),
    NotSvg,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Xml(e) => write!(f, "invalid SVG: {e}"),
            Error::NotSvg => write!(f, "not an SVG document"),
        }
    }
}

impl std::error::Error for Error {}

/// Quick sniff: an XML document whose root looks like `<svg`.
pub fn looks_like_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(2048)];
    let s = String::from_utf8_lossy(head);
    s.contains("<svg") && (s.trim_start().starts_with('<') || s.starts_with('\u{feff}'))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FillRule {
    #[default]
    NonZero,
    EvenOdd,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Cap {
    #[default]
    Butt,
    Round,
    Square,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Join {
    #[default]
    Miter,
    Round,
    Bevel,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Spread {
    #[default]
    Pad,
    Reflect,
    Repeat,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GradientKind {
    Linear { x1: f64, y1: f64, x2: f64, y2: f64 },
    Radial { cx: f64, cy: f64, r: f64, fx: f64, fy: f64 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    pub kind: GradientKind,
    /// (offset 0..1, straight RGBA 0..1), sorted.
    pub stops: Vec<(f64, [f64; 4])>,
    /// `gradientUnits="objectBoundingBox"` (the default): coordinates are fractions of the
    /// shape's bounding box.
    pub bbox_units: bool,
    pub transform: Affine,
    pub spread: Spread,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Paint {
    Color([f64; 3]),
    Gradient(Gradient),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stroke {
    pub paint: Paint,
    pub opacity: f64,
    pub width: f64,
    pub cap: Cap,
    pub join: Join,
    pub miter: f64,
    /// Dash array and offset.
    pub dash: Option<(Vec<f64>, f64)>,
}

/// Shape geometry in its user space (basic shapes are kept so they can become parametric shape
/// layer items).
#[derive(Clone, Debug, PartialEq)]
pub enum Geom {
    Rect { x: f64, y: f64, w: f64, h: f64, rx: f64, ry: f64 },
    Ellipse { cx: f64, cy: f64, rx: f64, ry: f64 },
    Path(BezPath),
}

impl Geom {
    pub fn to_path(&self) -> BezPath {
        use kurbo::Shape as _;
        match self {
            Geom::Rect { x, y, w, h, rx, ry } => {
                let r = kurbo::Rect::new(*x, *y, x + w, y + h);
                if *rx > 0.0 || *ry > 0.0 {
                    if (rx - ry).abs() < 1e-9 { kurbo::RoundedRect::from_rect(r, *rx).to_path(0.01) } else { elliptic_round_rect(*x, *y, *w, *h, *rx, *ry) }
                } else {
                    r.to_path(0.01)
                }
            }
            Geom::Ellipse { cx, cy, rx, ry } => kurbo::Ellipse::new((*cx, *cy), (*rx, *ry), 0.0).to_path(0.01),
            Geom::Path(p) => p.clone(),
        }
    }
}

fn elliptic_round_rect(x: f64, y: f64, w: f64, h: f64, rx: f64, ry: f64) -> BezPath {
    const K: f64 = 0.552_284_749_830_793_6;
    let (kx, ky) = (rx * K, ry * K);
    let mut p = BezPath::new();
    p.move_to((x + rx, y));
    p.line_to((x + w - rx, y));
    p.curve_to((x + w - rx + kx, y), (x + w, y + ry - ky), (x + w, y + ry));
    p.line_to((x + w, y + h - ry));
    p.curve_to((x + w, y + h - ry + ky), (x + w - rx + kx, y + h), (x + w - rx, y + h));
    p.line_to((x + rx, y + h));
    p.curve_to((x + rx - kx, y + h), (x, y + h - ry + ky), (x, y + h - ry));
    p.line_to((x, y + ry));
    p.curve_to((x, y + ry - ky), (x + rx - kx, y), (x + rx, y));
    p.close_path();
    p
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    /// Element id, or the element name (`rect`, `path`…).
    pub name: String,
    pub geom: Geom,
    /// User space → parent space.
    pub transform: Affine,
    pub fill: Option<Paint>,
    pub fill_opacity: f64,
    pub fill_rule: FillRule,
    pub stroke: Option<Stroke>,
    pub opacity: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub name: String,
    pub transform: Affine,
    pub opacity: f64,
    pub children: Vec<Node>,
    /// Clipping paths in the group's space (after `transform`); the children show only where
    /// every one of them covers (PDF / EPS clipping; SVG `clipPath` is not read yet).
    pub clip: Vec<(BezPath, FillRule)>,
    /// How the group composites onto what is below it (PDF blend modes; SVG `mix-blend-mode`
    /// is not read yet).
    pub blend: BlendMode,
    /// A soft mask (PDF `SMask` in the graphics state): the group shows where the mask's
    /// luminosity (or alpha) is high.
    pub mask: Option<Box<SoftMask>>,
    /// Composited on its own (a transparent backdrop, as SVG groups and PDF isolated
    /// transparency groups are) rather than over what is below it (PDF non-isolated groups and
    /// clipping groups: blend modes inside reach the backdrop). Matters only when the group is
    /// drawn offscreen (clip, mask, blend mode, opacity or knockout).
    pub isolated: bool,
    /// A PDF knockout group: each child composites with the group's initial backdrop instead
    /// of with the children below it.
    pub knockout: bool,
}

impl Group {
    /// An empty, opaque, unclipped group.
    pub fn new(name: &str) -> Group {
        Group {
            name: name.to_string(),
            transform: Affine::IDENTITY,
            opacity: 1.0,
            children: vec![],
            clip: vec![],
            blend: BlendMode::Normal,
            mask: None,
            isolated: true,
            knockout: false,
        }
    }
}

/// Separable and non-separable blend modes (ISO 32000-1 §11.3.5; the same formulas as the
/// W3C Compositing and Blending Level 1 specification).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

impl BlendMode {
    /// A PDF blend mode name (`/Multiply` …); unknown names are Normal.
    pub fn from_pdf_name(n: &str) -> BlendMode {
        use BlendMode::*;
        match n {
            "Multiply" => Multiply,
            "Screen" => Screen,
            "Overlay" => Overlay,
            "Darken" => Darken,
            "Lighten" => Lighten,
            "ColorDodge" => ColorDodge,
            "ColorBurn" => ColorBurn,
            "HardLight" => HardLight,
            "SoftLight" => SoftLight,
            "Difference" => Difference,
            "Exclusion" => Exclusion,
            "Hue" => Hue,
            "Saturation" => Saturation,
            "Color" => Color,
            "Luminosity" => Luminosity,
            _ => Normal,
        }
    }
}

/// A soft mask: `content` is drawn in the masked group's space; its luminosity (composited
/// over `backdrop`) or its alpha is the mask.
#[derive(Clone, Debug, PartialEq)]
pub struct SoftMask {
    pub content: Group,
    pub luminosity: bool,
    pub backdrop: [f64; 3],
}

/// A raster image: `width`×`height` straight RGBA8 pixels; `transform` maps pixel space (y
/// down, pixel `(0, 0)` at the top-left corner) to the parent's space.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub name: String,
    pub transform: Affine,
    pub width: u32,
    pub height: u32,
    pub rgba: std::sync::Arc<Vec<u8>>,
    pub opacity: f64,
}

#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum Node {
    Group(Group),
    Shape(Shape),
    Image(Image),
}

/// A parsed SVG document. `root.transform` maps the viewBox to `width`×`height` pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Doc {
    pub width: f64,
    pub height: f64,
    pub root: Group,
    /// Elements that were skipped (text, image, filter…), by name.
    pub skipped: Vec<String>,
}

impl Doc {
    /// Pixel size (rounded up, at least 1×1).
    pub fn pixel_size(&self) -> (u32, u32) {
        (self.width.ceil().clamp(1.0, 30000.0) as u32, self.height.ceil().clamp(1.0, 30000.0) as u32)
    }
    /// Every shape with its accumulated transform (to document pixels) and opacity, in paint
    /// order.
    pub fn flatten(&self) -> Vec<(Affine, f64, &Shape)> {
        fn walk<'a>(g: &'a Group, m: Affine, o: f64, out: &mut Vec<(Affine, f64, &'a Shape)>) {
            let m = m * g.transform;
            let o = o * g.opacity;
            for c in &g.children {
                match c {
                    Node::Group(sub) => walk(sub, m, o, out),
                    Node::Shape(s) => out.push((m * s.transform, o * s.opacity, s)),
                    Node::Image(_) => {}
                }
            }
        }
        let mut out = vec![];
        walk(&self.root, Affine::IDENTITY, 1.0, &mut out);
        out
    }
}

// ---------------------------------------------------------------- styles

/// Inherited style state.
#[derive(Clone, Debug)]
struct Style {
    fill: Option<PaintRef>,
    fill_opacity: f64,
    fill_rule: FillRule,
    stroke: Option<PaintRef>,
    stroke_opacity: f64,
    stroke_width: f64,
    cap: Cap,
    join: Join,
    miter: f64,
    dash: Option<Vec<f64>>,
    dash_offset: f64,
    color: [f64; 4],
    visible: bool,
    font_size: f64,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            fill: Some(PaintRef::Color([0.0, 0.0, 0.0, 1.0])),
            fill_opacity: 1.0,
            fill_rule: FillRule::NonZero,
            stroke: None,
            stroke_opacity: 1.0,
            stroke_width: 1.0,
            cap: Cap::Butt,
            join: Join::Miter,
            miter: 4.0,
            dash: None,
            dash_offset: 0.0,
            color: [0.0, 0.0, 0.0, 1.0],
            visible: true,
            font_size: 16.0,
        }
    }
}

#[derive(Clone, Debug)]
enum PaintRef {
    Color([f64; 4]),
    Url(String, Option<[f64; 4]>),
    Current,
}

/// A CSS rule: selector (type, class, id; `*`) with its specificity and declarations.
#[derive(Clone, Debug)]
struct Rule {
    tag: Option<String>,
    classes: Vec<String>,
    id: Option<String>,
    specificity: u32,
    order: usize,
    decls: Vec<(String, String)>,
}

fn parse_decls(s: &str) -> Vec<(String, String)> {
    s.split(';')
        .filter_map(|d| {
            let (k, v) = d.split_once(':')?;
            let v = v.trim().trim_end_matches("!important").trim();
            Some((k.trim().to_ascii_lowercase(), v.to_string()))
        })
        .filter(|(k, _)| !k.is_empty())
        .collect()
}

fn strip_comments(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("/*") {
        out.push_str(&rest[..i]);
        match rest[i + 2..].find("*/") {
            Some(j) => rest = &rest[i + 2 + j + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

fn parse_css(css: &str, rules: &mut Vec<Rule>) {
    let css = strip_comments(css);
    let mut rest = css.as_str();
    while let Some(open) = rest.find('{') {
        let sel = rest[..open].trim();
        let Some(close) = rest[open..].find('}') else { break };
        let body = &rest[open + 1..open + close];
        rest = &rest[open + close + 1..];
        if sel.starts_with('@') {
            continue;
        }
        let decls = parse_decls(body);
        for one in sel.split(',') {
            let one = one.trim();
            // Only simple selectors (no combinators / pseudo-classes / attributes).
            if one.is_empty() || one.contains([' ', '>', '+', '~', ':', '[']) {
                continue;
            }
            let mut tag = None;
            let mut classes = vec![];
            let mut id = None;
            let mut cur = String::new();
            let mut kind = 't';
            let flush = |kind: char, cur: &mut String, tag: &mut Option<String>, classes: &mut Vec<String>, id: &mut Option<String>| {
                if !cur.is_empty() {
                    match kind {
                        '.' => classes.push(std::mem::take(cur)),
                        '#' => *id = Some(std::mem::take(cur)),
                        _ => {
                            if cur != "*" {
                                *tag = Some(std::mem::take(cur));
                            }
                            cur.clear();
                        }
                    }
                }
            };
            for c in one.chars() {
                if c == '.' || c == '#' {
                    flush(kind, &mut cur, &mut tag, &mut classes, &mut id);
                    kind = c;
                } else {
                    cur.push(c);
                }
            }
            flush(kind, &mut cur, &mut tag, &mut classes, &mut id);
            let specificity = id.is_some() as u32 * 100 + classes.len() as u32 * 10 + tag.is_some() as u32;
            let order = rules.len();
            rules.push(Rule { tag, classes, id, specificity, order, decls: decls.clone() });
        }
    }
}

const PROPS: &[&str] = &[
    "fill",
    "fill-opacity",
    "fill-rule",
    "stroke",
    "stroke-width",
    "stroke-opacity",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-miterlimit",
    "stroke-dasharray",
    "stroke-dashoffset",
    "opacity",
    "display",
    "visibility",
    "color",
    "font-size",
    "transform",
    "stop-color",
    "stop-opacity",
];

struct Parser<'a, 'i> {
    ids: HashMap<String, roxmltree::Node<'a, 'i>>,
    rules: Vec<Rule>,
    vw: f64,
    vh: f64,
    skipped: Vec<String>,
}

fn is_svg_ns(n: &roxmltree::Node) -> bool {
    n.is_element()
}

impl<'a, 'i> Parser<'a, 'i> {
    /// The declared properties of an element: presentation attributes, then CSS rules (by
    /// specificity), then the `style` attribute.
    fn declared(&self, n: roxmltree::Node) -> HashMap<String, String> {
        let mut out = HashMap::new();
        for a in n.attributes() {
            if PROPS.contains(&a.name()) && a.name() != "transform" {
                out.insert(a.name().to_string(), a.value().to_string());
            }
        }
        let tag = n.tag_name().name();
        let classes: Vec<&str> = n.attribute("class").map(|c| c.split_whitespace().collect()).unwrap_or_default();
        let id = n.attribute("id");
        let mut matching: Vec<&Rule> = self
            .rules
            .iter()
            .filter(|r| {
                r.tag.as_deref().is_none_or(|t| t == tag)
                    && r.id.as_deref().is_none_or(|i| Some(i) == id)
                    && r.classes.iter().all(|c| classes.contains(&c.as_str()))
            })
            .collect();
        matching.sort_by_key(|r| (r.specificity, r.order));
        for r in matching {
            for (k, v) in &r.decls {
                out.insert(k.clone(), v.clone());
            }
        }
        if let Some(st) = n.attribute("style") {
            for (k, v) in parse_decls(st) {
                out.insert(k, v);
            }
        }
        out
    }

    fn length(&self, s: &str, axis: char, font: f64) -> Option<f64> {
        let s = s.trim();
        let num_end = s.find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E'))).unwrap_or(s.len());
        // "e" also starts "em"/"ex": back off if the number does not parse.
        let (mut num, mut unit) = (&s[..num_end], &s[num_end..]);
        if num.parse::<f64>().is_err() && (num.ends_with('e') || num.ends_with('E')) {
            num = &s[..num_end - 1];
            unit = &s[num_end - 1..];
        }
        let v: f64 = num.parse().ok()?;
        Some(match unit.trim() {
            "" | "px" => v,
            "pt" => v * 4.0 / 3.0,
            "pc" => v * 16.0,
            "mm" => v * 96.0 / 25.4,
            "cm" => v * 96.0 / 2.54,
            "in" => v * 96.0,
            "em" => v * font,
            "ex" => v * font / 2.0,
            "%" => {
                let base = match axis {
                    'x' => self.vw,
                    'y' => self.vh,
                    _ => ((self.vw * self.vw + self.vh * self.vh) / 2.0).sqrt(),
                };
                v / 100.0 * base
            }
            _ => v,
        })
    }

    fn attr_len(&self, n: roxmltree::Node, name: &str, axis: char, st: &Style) -> f64 {
        n.attribute(name).and_then(|v| self.length(v, axis, st.font_size)).unwrap_or(0.0)
    }

    fn paint_ref(&self, v: &str) -> Option<Option<PaintRef>> {
        let v = v.trim();
        if v == "none" {
            return Some(None);
        }
        if v == "currentColor" || v == "currentcolor" {
            return Some(Some(PaintRef::Current));
        }
        if let Some(rest) = v.strip_prefix("url(") {
            let (url, fallback) = rest.split_once(')').unwrap_or((rest, ""));
            let id = url.trim().trim_matches(['"', '\'']).trim_start_matches('#').to_string();
            let fb = fallback.trim();
            let fb = if fb.is_empty() || fb == "none" { None } else { parse_color(fb) };
            return Some(Some(PaintRef::Url(id, fb)));
        }
        if v == "inherit" {
            return None;
        }
        parse_color(v).map(|c| Some(PaintRef::Color(c)))
    }

    /// Apply an element's declared inherited properties to `st`. Returns (opacity, display).
    fn cascade(&self, n: roxmltree::Node, st: &mut Style) -> (f64, bool) {
        let d = self.declared(n);
        let num =
            |k: &str| d.get(k).and_then(|v| v.trim().trim_end_matches('%').parse::<f64>().ok().map(|x| if v.trim().ends_with('%') { x / 100.0 } else { x }));
        if let Some(c) = d.get("color").and_then(|v| parse_color(v)) {
            st.color = c;
        }
        if let Some(v) = d.get("font-size").and_then(|v| self.length(v, 'o', st.font_size)) {
            st.font_size = v;
        }
        if let Some(p) = d.get("fill").and_then(|v| self.paint_ref(v)) {
            st.fill = p;
        }
        if let Some(p) = d.get("stroke").and_then(|v| self.paint_ref(v)) {
            st.stroke = p;
        }
        if let Some(v) = num("fill-opacity") {
            st.fill_opacity = v.clamp(0.0, 1.0);
        }
        if let Some(v) = num("stroke-opacity") {
            st.stroke_opacity = v.clamp(0.0, 1.0);
        }
        if let Some(v) = d.get("stroke-width").and_then(|v| self.length(v, 'o', st.font_size)) {
            st.stroke_width = v.max(0.0);
        }
        if let Some(v) = num("stroke-miterlimit") {
            st.miter = v.max(1.0);
        }
        if let Some(v) = d.get("fill-rule") {
            st.fill_rule = if v.trim() == "evenodd" { FillRule::EvenOdd } else { FillRule::NonZero };
        }
        if let Some(v) = d.get("stroke-linecap") {
            st.cap = match v.trim() {
                "round" => Cap::Round,
                "square" => Cap::Square,
                _ => Cap::Butt,
            };
        }
        if let Some(v) = d.get("stroke-linejoin") {
            st.join = match v.trim() {
                "round" => Join::Round,
                "bevel" => Join::Bevel,
                _ => Join::Miter,
            };
        }
        if let Some(v) = d.get("stroke-dasharray") {
            st.dash = if v.trim() == "none" {
                None
            } else {
                let a: Vec<f64> =
                    v.split(|c: char| c == ',' || c.is_whitespace()).filter(|t| !t.is_empty()).filter_map(|t| self.length(t, 'o', st.font_size)).collect();
                if a.iter().all(|x| *x >= 0.0) && a.iter().any(|x| *x > 0.0) { Some(a) } else { None }
            };
        }
        if let Some(v) = d.get("stroke-dashoffset").and_then(|v| self.length(v, 'o', st.font_size)) {
            st.dash_offset = v;
        }
        if let Some(v) = d.get("visibility") {
            st.visible = !matches!(v.trim(), "hidden" | "collapse");
        }
        let display = d.get("display").is_none_or(|v| v.trim() != "none");
        (num("opacity").unwrap_or(1.0).clamp(0.0, 1.0), display)
    }

    fn resolve_paint(&self, p: &PaintRef, st: &Style) -> Option<(Paint, f64)> {
        match p {
            PaintRef::Color(c) => Some((Paint::Color([c[0], c[1], c[2]]), c[3])),
            PaintRef::Current => Some((Paint::Color([st.color[0], st.color[1], st.color[2]]), st.color[3])),
            PaintRef::Url(id, fb) => match self.gradient(id, 0) {
                Some(g) if g.stops.len() == 1 => {
                    let c = g.stops[0].1;
                    Some((Paint::Color([c[0], c[1], c[2]]), c[3]))
                }
                Some(g) if !g.stops.is_empty() => Some((Paint::Gradient(g), 1.0)),
                Some(_) => None,
                None => fb.map(|c| (Paint::Color([c[0], c[1], c[2]]), c[3])),
            },
        }
    }

    fn href(&self, n: roxmltree::Node<'a, 'i>) -> Option<roxmltree::Node<'a, 'i>> {
        let h = n.attributes().find(|a| a.name() == "href").map(|a| a.value())?;
        self.ids.get(h.trim().trim_start_matches('#')).copied()
    }

    /// A gradient by id, with `href` inheritance of attributes and stops.
    fn gradient(&self, id: &str, depth: usize) -> Option<Gradient> {
        let n = *self.ids.get(id)?;
        let tag = n.tag_name().name();
        if !matches!(tag, "linearGradient" | "radialGradient") || depth > 16 {
            return None;
        }
        // Chain: this element, then what it references.
        let mut chain = vec![n];
        let mut cur = n;
        while let Some(next) = self.href(cur) {
            if chain.len() > 16 || chain.contains(&next) {
                break;
            }
            chain.push(next);
            cur = next;
        }
        let attr = |name: &str| chain.iter().find_map(|e| e.attribute(name));
        let bbox_units = attr("gradientUnits") != Some("userSpaceOnUse");
        let st = Style::default();
        let len = |name: &str, axis: char, def: &str| -> f64 {
            let v = attr(name).unwrap_or(def);
            if bbox_units {
                let v = v.trim();
                if let Some(p) = v.strip_suffix('%') { p.parse::<f64>().unwrap_or(0.0) / 100.0 } else { v.parse::<f64>().unwrap_or(0.0) }
            } else {
                self.length(v, axis, st.font_size).unwrap_or(0.0)
            }
        };
        let kind = if tag == "linearGradient" {
            GradientKind::Linear { x1: len("x1", 'x', "0%"), y1: len("y1", 'y', "0%"), x2: len("x2", 'x', "100%"), y2: len("y2", 'y', "0%") }
        } else {
            let cx = len("cx", 'x', "50%");
            let cy = len("cy", 'y', "50%");
            let r = len("r", 'o', "50%");
            let fx = if attr("fx").is_some() { len("fx", 'x', "50%") } else { cx };
            let fy = if attr("fy").is_some() { len("fy", 'y', "50%") } else { cy };
            GradientKind::Radial { cx, cy, r, fx, fy }
        };
        let transform = attr("gradientTransform").map(parse_transform).unwrap_or(Affine::IDENTITY);
        let spread = match attr("spreadMethod") {
            Some("reflect") => Spread::Reflect,
            Some("repeat") => Spread::Repeat,
            _ => Spread::Pad,
        };
        // Stops: from the first element in the chain that has any.
        let mut stops = vec![];
        if let Some(src) = chain.iter().find(|e| e.children().any(|c| c.is_element() && c.tag_name().name() == "stop")) {
            let mut last = 0.0f64;
            for s in src.children().filter(|c| c.is_element() && c.tag_name().name() == "stop") {
                let d = self.declared(s);
                let off = s
                    .attribute("offset")
                    .map(|o| {
                        let o = o.trim();
                        if let Some(p) = o.strip_suffix('%') { p.parse::<f64>().unwrap_or(0.0) / 100.0 } else { o.parse::<f64>().unwrap_or(0.0) }
                    })
                    .unwrap_or(0.0)
                    .clamp(0.0, 1.0)
                    .max(last);
                last = off;
                let c = d
                    .get("stop-color")
                    .and_then(|c| if c.trim() == "currentColor" { d.get("color").and_then(|x| parse_color(x)) } else { parse_color(c) })
                    .unwrap_or([0.0, 0.0, 0.0, 1.0]);
                let o = d.get("stop-opacity").and_then(|v| v.trim().parse::<f64>().ok()).unwrap_or(1.0).clamp(0.0, 1.0);
                stops.push((off, [c[0], c[1], c[2], c[3] * o]));
            }
        }
        Some(Gradient { kind, stops, bbox_units, transform, spread })
    }

    fn element_name(n: roxmltree::Node) -> String {
        n.attribute("id").map(str::to_string).unwrap_or_else(|| n.tag_name().name().to_string())
    }

    /// Convert an element (and its subtree) into nodes appended to `out`.
    fn node(&mut self, n: roxmltree::Node<'a, 'i>, parent: &Style, out: &mut Vec<Node>, depth: usize) {
        if depth > 64 || !is_svg_ns(&n) {
            return;
        }
        let tag = n.tag_name().name();
        match tag {
            "defs" | "title" | "desc" | "metadata" | "style" | "linearGradient" | "radialGradient" | "symbol" | "clipPath" | "mask" | "pattern" | "marker"
            | "script" => return,
            "text" | "image" | "filter" | "foreignObject" | "switch" => {
                self.skipped.push(tag.to_string());
                return;
            }
            _ => {}
        }
        let mut st = parent.clone();
        let (opacity, display) = self.cascade(n, &mut st);
        if !display {
            return;
        }
        let mut transform = n.attribute("transform").map(parse_transform).unwrap_or(Affine::IDENTITY);
        match tag {
            "g" | "a" => {
                let mut children = vec![];
                for c in n.children() {
                    self.node(c, &st, &mut children, depth + 1);
                }
                out.push(Node::Group(Group {
                    name: Self::element_name(n),
                    transform,
                    opacity,
                    children,
                    clip: vec![],
                    blend: BlendMode::Normal,
                    mask: None,
                    isolated: true,
                    knockout: false,
                }));
            }
            "svg" => {
                // Nested viewport.
                let x = self.attr_len(n, "x", 'x', &st);
                let y = self.attr_len(n, "y", 'y', &st);
                let w = n.attribute("width").and_then(|v| self.length(v, 'x', st.font_size)).filter(|v| v.is_finite()).unwrap_or(self.vw);
                let h = n.attribute("height").and_then(|v| self.length(v, 'y', st.font_size)).filter(|v| v.is_finite()).unwrap_or(self.vh);
                let vb = viewbox_transform(n, w, h);
                let mut children = vec![];
                let saved = (self.vw, self.vh);
                // Percentages use this viewport's user space, even without a viewBox.
                let (vw, vh) = viewbox(n).map_or((w, h), |(_, vw, vh)| (vw, vh));
                self.vw = vw;
                self.vh = vh;
                for c in n.children() {
                    self.node(c, &st, &mut children, depth + 1);
                }
                (self.vw, self.vh) = saved;
                out.push(Node::Group(Group {
                    name: Self::element_name(n),
                    transform: Affine::translate((x, y)) * vb,
                    opacity,
                    children,
                    clip: vec![],
                    blend: BlendMode::Normal,
                    mask: None,
                    isolated: true,
                    knockout: false,
                }));
            }
            "use" => {
                let Some(target) = self.href(n) else { return };
                if target == n || n.ancestors().any(|a| a == target) {
                    return;
                }
                let x = self.attr_len(n, "x", 'x', &st);
                let y = self.attr_len(n, "y", 'y', &st);
                transform *= Affine::translate((x, y));
                let mut children = vec![];
                if target.tag_name().name() == "symbol" {
                    let w = n.attribute("width").and_then(|v| self.length(v, 'x', st.font_size)).unwrap_or(self.vw);
                    let h = n.attribute("height").and_then(|v| self.length(v, 'y', st.font_size)).unwrap_or(self.vh);
                    let vb = viewbox_transform(target, w, h);
                    let mut inner = vec![];
                    let mut sst = st.clone();
                    let (o, _) = self.cascade(target, &mut sst);
                    for c in target.children() {
                        self.node(c, &sst, &mut inner, depth + 1);
                    }
                    children.push(Node::Group(Group {
                        name: Self::element_name(target),
                        transform: vb,
                        opacity: o,
                        children: inner,
                        clip: vec![],
                        blend: BlendMode::Normal,
                        mask: None,
                        isolated: true,
                        knockout: false,
                    }));
                } else {
                    self.node(target, &st, &mut children, depth + 1);
                }
                out.push(Node::Group(Group {
                    name: Self::element_name(n),
                    transform,
                    opacity,
                    children,
                    clip: vec![],
                    blend: BlendMode::Normal,
                    mask: None,
                    isolated: true,
                    knockout: false,
                }));
            }
            "path" | "rect" | "circle" | "ellipse" | "line" | "polyline" | "polygon" => {
                if !st.visible {
                    return;
                }
                let Some(geom) = self.geometry(n, &st) else { return };
                let fill = if matches!(tag, "line") { None } else { st.fill.as_ref().and_then(|p| self.resolve_paint(p, &st)) };
                let (fill, fill_alpha) = match fill {
                    Some((p, a)) => (Some(p), a),
                    None => (None, 1.0),
                };
                let stroke = st.stroke.as_ref().and_then(|p| self.resolve_paint(p, &st)).filter(|_| st.stroke_width > 0.0).map(|(paint, a)| Stroke {
                    paint,
                    opacity: st.stroke_opacity * a,
                    width: st.stroke_width,
                    cap: st.cap,
                    join: st.join,
                    miter: st.miter,
                    dash: st.dash.clone().map(|d| (d, st.dash_offset)),
                });
                out.push(Node::Shape(Shape {
                    name: Self::element_name(n),
                    geom,
                    transform,
                    fill,
                    fill_opacity: st.fill_opacity * fill_alpha,
                    fill_rule: st.fill_rule,
                    stroke,
                    opacity,
                }));
            }
            _ => {
                // Unknown container-ish elements: render their children.
                if n.has_children() {
                    let mut children = vec![];
                    for c in n.children() {
                        self.node(c, &st, &mut children, depth + 1);
                    }
                    if !children.is_empty() {
                        out.push(Node::Group(Group {
                            name: Self::element_name(n),
                            transform,
                            opacity,
                            children,
                            clip: vec![],
                            blend: BlendMode::Normal,
                            mask: None,
                            isolated: true,
                            knockout: false,
                        }));
                    }
                }
            }
        }
    }

    fn geometry(&self, n: roxmltree::Node, st: &Style) -> Option<Geom> {
        let l = |name: &str, axis: char| self.attr_len(n, name, axis, st);
        Some(match n.tag_name().name() {
            "path" => {
                let p = parse_path_data(n.attribute("d")?);
                if p.elements().is_empty() {
                    return None;
                }
                Geom::Path(p)
            }
            "rect" => {
                let (w, h) = (l("width", 'x'), l("height", 'y'));
                if !(w > 0.0 && h > 0.0) {
                    return None;
                }
                let rx_a = n.attribute("rx").and_then(|v| self.length(v, 'x', st.font_size));
                let ry_a = n.attribute("ry").and_then(|v| self.length(v, 'y', st.font_size));
                let (rx, ry) = match (rx_a, ry_a) {
                    (Some(x), Some(y)) => (x, y),
                    (Some(x), None) => (x, x),
                    (None, Some(y)) => (y, y),
                    (None, None) => (0.0, 0.0),
                };
                Geom::Rect { x: l("x", 'x'), y: l("y", 'y'), w, h, rx: rx.clamp(0.0, w / 2.0), ry: ry.clamp(0.0, h / 2.0) }
            }
            "circle" => {
                let r = l("r", 'o');
                if r <= 0.0 {
                    return None;
                }
                Geom::Ellipse { cx: l("cx", 'x'), cy: l("cy", 'y'), rx: r, ry: r }
            }
            "ellipse" => {
                let (rx, ry) = (l("rx", 'x'), l("ry", 'y'));
                if rx <= 0.0 || ry <= 0.0 {
                    return None;
                }
                Geom::Ellipse { cx: l("cx", 'x'), cy: l("cy", 'y'), rx, ry }
            }
            "line" => {
                let mut p = BezPath::new();
                p.move_to((l("x1", 'x'), l("y1", 'y')));
                p.line_to((l("x2", 'x'), l("y2", 'y')));
                Geom::Path(p)
            }
            "polyline" | "polygon" => {
                let nums = pathdata::numbers(n.attribute("points")?);
                if nums.len() < 4 {
                    return None;
                }
                let mut p = BezPath::new();
                p.move_to((nums[0], nums[1]));
                for c in nums[2..].as_chunks::<2>().0 {
                    p.line_to((c[0], c[1]));
                }
                if n.tag_name().name() == "polygon" {
                    p.close_path();
                }
                Geom::Path(p)
            }
            _ => return None,
        })
    }
}

fn viewbox(n: roxmltree::Node) -> Option<([f64; 2], f64, f64)> {
    let v = pathdata::numbers(n.attribute("viewBox")?);
    (v.len() == 4 && v.iter().all(|x| x.is_finite()) && v[2] > 0.0 && v[3] > 0.0).then(|| ([v[0], v[1]], v[2], v[3]))
}

/// The viewBox → viewport transform (`preserveAspectRatio`, default `xMidYMid meet`).
fn viewbox_transform(n: roxmltree::Node, w: f64, h: f64) -> Affine {
    let Some(([x, y], vw, vh)) = viewbox(n) else { return Affine::IDENTITY };
    let par = n.attribute("preserveAspectRatio").unwrap_or("xMidYMid meet");
    let mut parts = par.split_whitespace();
    let mut align = parts.next().unwrap_or("xMidYMid");
    if align == "defer" {
        align = parts.next().unwrap_or("xMidYMid");
    }
    let slice = parts.next() == Some("slice");
    let (sx, sy) = (w / vw, h / vh);
    if align == "none" {
        return Affine::scale_non_uniform(sx, sy) * Affine::translate((-x, -y));
    }
    let s = if slice { sx.max(sy) } else { sx.min(sy) };
    let fx = if align.contains("xMid") {
        0.5
    } else if align.contains("xMax") {
        1.0
    } else {
        0.0
    };
    let fy = if align.contains("YMid") {
        0.5
    } else if align.contains("YMax") {
        1.0
    } else {
        0.0
    };
    let tx = (w - vw * s) * fx;
    let ty = (h - vh * s) * fy;
    Affine::translate((tx, ty)) * Affine::scale(s) * Affine::translate((-x, -y))
}

/// Parse a `transform` list.
pub fn parse_transform(s: &str) -> Affine {
    let mut m = Affine::IDENTITY;
    let mut rest = s;
    while let Some(open) = rest.find('(') {
        let name = rest[..open].trim().trim_start_matches(',').trim();
        let Some(close) = rest[open..].find(')') else { break };
        let a = pathdata::numbers(&rest[open + 1..open + close]);
        rest = &rest[open + close + 1..];
        let t = match (name, a.len()) {
            ("matrix", 6) => Affine::new([a[0], a[1], a[2], a[3], a[4], a[5]]),
            ("translate", 1) => Affine::translate((a[0], 0.0)),
            ("translate", 2) => Affine::translate((a[0], a[1])),
            ("scale", 1) => Affine::scale(a[0]),
            ("scale", 2) => Affine::scale_non_uniform(a[0], a[1]),
            ("rotate", 1) => Affine::rotate(a[0].to_radians()),
            ("rotate", 3) => Affine::translate((a[1], a[2])) * Affine::rotate(a[0].to_radians()) * Affine::translate((-a[1], -a[2])),
            ("skewX", 1) => Affine::new([1.0, 0.0, a[0].to_radians().tan(), 1.0, 0.0, 0.0]),
            ("skewY", 1) => Affine::new([1.0, a[0].to_radians().tan(), 0.0, 1.0, 0.0, 0.0]),
            _ => Affine::IDENTITY,
        };
        m *= t;
    }
    m
}

/// The deepest element nesting accepted. The XML parser recurses per level and overflowed a
/// 2 MiB thread's stack (aborting the app) at a few thousand levels; real documents stay far
/// below this.
const MAX_XML_DEPTH: usize = 256;

/// The deepest element nesting of `text` (a quick scan that skips comments, CDATA, processing
/// instructions, declarations and quoted attribute values).
fn xml_depth(text: &str) -> usize {
    let b = text.as_bytes();
    let (mut i, mut depth, mut max) = (0usize, 0usize, 0usize);
    let skip_to = |from: usize, end: &[u8]| b.get(from..).and_then(|r| r.windows(end.len()).position(|w| w == end)).map_or(b.len(), |p| from + p + end.len());
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        let rest = &b[i..];
        if rest.starts_with(b"<!--") {
            i = skip_to(i + 4, b"-->");
        } else if rest.starts_with(b"<![CDATA[") {
            i = skip_to(i + 9, b"]]>");
        } else if rest.starts_with(b"<?") {
            i = skip_to(i + 2, b"?>");
        } else if rest.starts_with(b"<!") {
            i = skip_to(i + 2, b">");
        } else if rest.starts_with(b"</") {
            depth = depth.saturating_sub(1);
            i = skip_to(i + 2, b">");
        } else {
            // A start tag: find its end outside quotes; `/>` closes it at once.
            let mut j = i + 1;
            let mut quote = None;
            while let Some(&c) = b.get(j) {
                match quote {
                    Some(q) if c == q => quote = None,
                    Some(_) => {}
                    None if c == b'"' || c == b'\'' => quote = Some(c),
                    None if c == b'>' => break,
                    None => {}
                }
                j += 1;
            }
            if b.get(j.wrapping_sub(1)) != Some(&b'/') {
                depth += 1;
                max = max.max(depth);
            }
            i = j + 1;
        }
    }
    max
}

/// Parse an SVG document.
pub fn parse(bytes: &[u8]) -> Result<Doc, Error> {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim_start_matches('\u{feff}');
    if xml_depth(text) > MAX_XML_DEPTH {
        return Err(Error::Xml(format!("elements nested more than {MAX_XML_DEPTH} levels deep")));
    }
    let opts = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    let xml = roxmltree::Document::parse_with_options(text, opts).map_err(|e| Error::Xml(e.to_string()))?;
    let root = xml.root_element();
    if root.tag_name().name() != "svg" {
        return Err(Error::NotSvg);
    }
    let mut ids = HashMap::new();
    let mut rules = vec![];
    for n in root.descendants().filter(|n| n.is_element()) {
        if let Some(id) = n.attribute("id") {
            ids.entry(id.to_string()).or_insert(n);
        }
        if n.tag_name().name() == "style" {
            let css: String = n.children().filter_map(|c| c.text()).collect();
            parse_css(&css, &mut rules);
        }
    }
    let vb = viewbox(root);
    let mut p = Parser { ids, rules, vw: vb.map_or(300.0, |v| v.1), vh: vb.map_or(150.0, |v| v.2), skipped: vec![] };
    let st = Style::default();
    let pct_or = |name: &str, axis: char, def: f64| -> f64 {
        match root.attribute(name) {
            Some(v) if v.trim().ends_with('%') => {
                let f = v.trim().trim_end_matches('%').parse::<f64>().unwrap_or(100.0) / 100.0;
                def * f
            }
            Some(v) => p.length(v, axis, st.font_size).filter(|x| *x > 0.0).unwrap_or(def),
            None => def,
        }
    };
    // Without width/height the viewBox size is used (or 300×150).
    let (dw, dh) = match vb {
        Some((_, w, h)) => {
            let w2 = pct_or("width", 'x', w);
            let h2 = match (root.attribute("width"), root.attribute("height")) {
                // Only width given: keep the aspect ratio.
                (Some(_), None) => w2 * h / w,
                _ => pct_or("height", 'y', h),
            };
            let w2 = if root.attribute("width").is_none() && root.attribute("height").is_some() { h2 * w / h } else { w2 };
            (w2, h2)
        }
        None => (pct_or("width", 'x', 300.0), pct_or("height", 'y', 150.0)),
    };
    // Percentages use the root viewport's user space, even without a viewBox. A size that is not
    // a finite positive number keeps the 300x150 default, so it cannot produce NaN geometry.
    let usable = |v: f64, d: f64| if v.is_finite() && v > 0.0 { v } else { d };
    (p.vw, p.vh) = vb.map_or((usable(dw, 300.0), usable(dh, 150.0)), |(_, w, h)| (w, h));
    let mut rst = st.clone();
    let (opacity, _) = p.cascade(root, &mut rst);
    let mut children = vec![];
    for c in root.children() {
        p.node(c, &rst, &mut children, 1);
    }
    let transform = viewbox_transform(root, dw, dh);
    Ok(Doc {
        width: dw,
        height: dh,
        root: Group { name: "svg".into(), transform, opacity, children, clip: vec![], blend: BlendMode::Normal, mask: None, isolated: true, knockout: false },
        skipped: std::mem::take(&mut p.skipped),
    })
}

#[cfg(test)]
mod tests;
