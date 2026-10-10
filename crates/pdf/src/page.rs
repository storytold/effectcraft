//! Pages and content streams (ISO 32000-1 §7.7.3, §8): the page tree, the graphics state,
//! path construction and painting, clipping, colour, shading patterns and `sh`, form XObjects
//! and optional-content marked sequences, text (§9: text state, text objects, glyph outlines
//! from [`crate::font`]), images ([`crate::image`]), tiling patterns, blend modes and soft
//! masks. What is not drawn is listed in [`effectcraft_svg::Doc::skipped`].

use std::collections::HashMap;
use std::rc::Rc;

use effectcraft_svg::{
    Affine, BezPath, BlendMode, Cap, FillRule, Gradient, GradientKind, Group, Image as SvgImage, Join, Node, Paint, SoftMask, Spread, Stroke,
};
use kurbo::{Point, Shape as _};

use crate::build::Builder;
use crate::color::{Cs, Func, color_space, text_string};
use crate::font::Font;
use crate::object::{Dict, File, Lexer, Obj, decode_stream};

/// A page: its dictionary with inherited attributes resolved.
pub struct Page {
    pub dict: Dict,
    pub resources: Dict,
    /// CropBox (else MediaBox) `[x0, y0, x1, y1]`.
    pub bbox: [f64; 4],
    pub rotate: i32,
}

/// The pages in order.
pub fn pages(file: &File) -> Vec<Page> {
    let mut out = vec![];
    let Some(cat) = file.catalog() else { return out };
    let Some(root) = file.get_dict(cat, "Pages") else { return out };
    fn walk(file: &File, node: &Dict, inherited: (Option<Dict>, Option<Vec<f64>>, Option<Vec<f64>>, i32), out: &mut Vec<Page>, depth: usize) {
        if depth > 32 || out.len() > 10_000 {
            return;
        }
        let res = file.get_dict(node, "Resources").cloned().or(inherited.0);
        let media = file.get(node, "MediaBox").map(|m| file.nums(m)).filter(|v| v.len() == 4).or(inherited.1);
        let crop = file.get(node, "CropBox").map(|m| file.nums(m)).filter(|v| v.len() == 4).or(inherited.2);
        let rot = file.get_num(node, "Rotate").map(|r| r as i32).unwrap_or(inherited.3);
        match file.get(node, "Kids").and_then(Obj::array) {
            Some(kids) if file.get(node, "Type").and_then(Obj::name) != Some("Page") => {
                for k in kids {
                    if let Some(d) = file.resolve(k).dict() {
                        walk(file, d, (res.clone(), media.clone(), crop.clone(), rot), out, depth + 1);
                    }
                }
            }
            _ => {
                let b = crop.or(media).unwrap_or_else(|| vec![0.0, 0.0, 612.0, 792.0]);
                let bbox = [b[0].min(b[2]), b[1].min(b[3]), b[0].max(b[2]), b[1].max(b[3])];
                out.push(Page { dict: node.clone(), resources: res.unwrap_or_default(), bbox, rotate: rot.rem_euclid(360) });
            }
        }
    }
    walk(file, root, (None, None, None, 0), &mut out, 0);
    out
}

/// The page's content streams, decoded and joined.
pub fn page_content(file: &File, page: &Page) -> Vec<u8> {
    let mut out = vec![];
    let parts: Vec<&Obj> = match page.dict.get("Contents").map(|c| file.resolve(c)) {
        Some(Obj::Array(a)) => a.iter().map(|x| file.resolve(x)).collect(),
        Some(o) => vec![o],
        None => vec![],
    };
    for p in parts {
        if let Obj::Stream(d, raw) = p
            && let Some(data) = decode_stream(file, d, raw)
        {
            out.extend_from_slice(&data);
            out.push(b'\n');
        }
    }
    out
}

/// A tiling pattern (§8.7.3.1): its cell content, drawn in pattern space at every step.
pub(crate) struct Tiling {
    /// Pattern space → page (default user space of the stream that set it).
    space: Affine,
    bbox: [f64; 4],
    xstep: f64,
    ystep: f64,
    content: Vec<u8>,
    res: Dict,
    /// PaintType 2: the cell is painted with this colour.
    uncolored: Option<[f64; 3]>,
}

/// A shading pattern without a gradient equivalent (function-based or mesh shadings),
/// rasterised when it fills a path.
pub(crate) struct ShadingFill {
    sh: Obj,
    /// Shading space → page.
    space: Affine,
}

#[derive(Clone)]
enum Fill {
    Color([f64; 3]),
    Paint(Paint),
    Tiling(Rc<Tiling>),
    Shading(Rc<ShadingFill>),
    None,
}

#[derive(Clone)]
struct GState {
    ctm: Affine,
    fill_cs: Cs,
    stroke_cs: Cs,
    fill: Fill,
    stroke: Fill,
    lw: f64,
    cap: Cap,
    join: Join,
    miter: f64,
    dash: Option<(Vec<f64>, f64)>,
    fill_alpha: f64,
    stroke_alpha: f64,
    // Text state (§9.3).
    font: Option<Rc<Font>>,
    font_size: f64,
    char_spacing: f64,
    word_spacing: f64,
    h_scale: f64,
    leading: f64,
    rise: f64,
    render_mode: u8,
    /// Builder depths of the open blend-mode and soft-mask groups.
    blend_depth: Option<usize>,
    mask_depth: Option<usize>,
    /// Colour operators are ignored (uncoloured tiling patterns, `d1` glyphs).
    lock_color: bool,
}

impl Default for GState {
    fn default() -> Self {
        GState {
            ctm: Affine::IDENTITY,
            fill_cs: Cs::Gray,
            stroke_cs: Cs::Gray,
            fill: Fill::Color([0.0; 3]),
            stroke: Fill::Color([0.0; 3]),
            lw: 1.0,
            cap: Cap::Butt,
            join: Join::Miter,
            miter: 10.0,
            dash: None,
            fill_alpha: 1.0,
            stroke_alpha: 1.0,
            font: None,
            font_size: 0.0,
            char_spacing: 0.0,
            word_spacing: 0.0,
            h_scale: 1.0,
            leading: 0.0,
            rise: 0.0,
            render_mode: 0,
            blend_depth: None,
            mask_depth: None,
            lock_color: false,
        }
    }
}

pub(crate) struct Interp<'a> {
    file: &'a File,
    pub b: Builder,
    gs: GState,
    saved: Vec<(GState, usize)>,
    path: BezPath,
    cur: Option<Point>,
    start: Option<Point>,
    pending_clip: Option<FillRule>,
    marked: Vec<Option<usize>>,
    /// The page box in default user space (for `sh`).
    page_box: [f64; 4],
    budget: usize,
    // Text object state.
    tm: Affine,
    tlm: Affine,
    /// Glyph outlines collected for a clipping text render mode (page space).
    text_clip: Option<BezPath>,
    fonts: HashMap<String, Rc<Font>>,
}

fn affine(v: &[f64]) -> Affine {
    if v.len() < 6 {
        return Affine::IDENTITY;
    }
    Affine::new([v[0], v[1], v[2], v[3], v[4], v[5]])
}

/// The most tiles one tiling-pattern fill draws.
const MAX_TILES: usize = 4096;

/// A `TJ` element.
enum TextItem {
    Str(Vec<u8>),
    Adjust(f64),
}

impl<'a> Interp<'a> {
    pub fn new(file: &'a File, page_box: [f64; 4]) -> Self {
        Interp {
            file,
            b: Builder::new(),
            gs: GState::default(),
            saved: vec![],
            path: BezPath::new(),
            cur: None,
            start: None,
            pending_clip: None,
            marked: vec![],
            page_box,
            budget: 20_000_000,
            tm: Affine::IDENTITY,
            tlm: Affine::IDENTITY,
            text_clip: None,
            fonts: HashMap::new(),
        }
    }

    /// Close builder groups down to `depth` and forget graphics-state groups above it.
    fn close_to(&mut self, depth: usize) {
        self.b.close_to(depth);
        if self.gs.blend_depth.is_some_and(|d| d >= depth) {
            self.gs.blend_depth = None;
        }
        if self.gs.mask_depth.is_some_and(|d| d >= depth) {
            self.gs.mask_depth = None;
        }
    }

    /// Run a content stream with resources `res`; `pattern_base` maps pattern space to the page
    /// (the CTM where the stream's default coordinate system starts).
    pub fn run(&mut self, content: &[u8], res: &Dict, pattern_base: Affine, depth: usize) {
        let mut lx = Lexer::new(content, 0);
        let mut args: Vec<Obj> = Vec::with_capacity(8);
        while let Some(o) = lx.next() {
            let Obj::Op(op) = o else {
                if args.len() < 64 {
                    args.push(o);
                }
                continue;
            };
            if self.budget == 0 {
                self.b.skip("content (operation limit)");
                return;
            }
            self.budget -= 1;
            let n = |i: usize| args.get(i).and_then(Obj::num).unwrap_or(0.0);
            let nums = || args.iter().filter_map(Obj::num).collect::<Vec<f64>>();
            let locked = self.gs.lock_color;
            match op.as_str() {
                "q" => self.saved.push((self.gs.clone(), self.b.depth())),
                "Q" => {
                    if let Some((g, d)) = self.saved.pop() {
                        self.gs = g;
                        self.close_to(d);
                    }
                }
                "cm" => self.gs.ctm *= affine(&nums()),
                "w" => self.gs.lw = n(0),
                "J" => self.gs.cap = [Cap::Butt, Cap::Round, Cap::Square][(n(0) as usize).min(2)],
                "j" => self.gs.join = [Join::Miter, Join::Round, Join::Bevel][(n(0) as usize).min(2)],
                "M" => self.gs.miter = n(0),
                "d" => {
                    let arr = args.first().map(|a| self.file.nums(a)).unwrap_or_default();
                    self.gs.dash = (!arr.is_empty() && arr.iter().any(|v| *v > 0.0)).then(|| (arr, n(1)));
                }
                "gs" => {
                    if let Some(name) = args.first().and_then(Obj::name)
                        && let Some(g) = self.file.get_dict(res, "ExtGState").and_then(|e| self.file.get_dict(e, name))
                    {
                        let g = g.clone();
                        self.ext_gstate(&g, res, depth);
                    }
                }
                // Path construction.
                "m" => {
                    let p = Point::new(n(0), n(1));
                    self.path.move_to(p);
                    self.cur = Some(p);
                    self.start = Some(p);
                }
                "l" => {
                    let p = Point::new(n(0), n(1));
                    self.ensure_start();
                    self.path.line_to(p);
                    self.cur = Some(p);
                }
                "c" => {
                    self.ensure_start();
                    let p = Point::new(n(4), n(5));
                    self.path.curve_to(Point::new(n(0), n(1)), Point::new(n(2), n(3)), p);
                    self.cur = Some(p);
                }
                "v" => {
                    self.ensure_start();
                    let c0 = self.cur.unwrap_or_default();
                    let p = Point::new(n(2), n(3));
                    self.path.curve_to(c0, Point::new(n(0), n(1)), p);
                    self.cur = Some(p);
                }
                "y" => {
                    self.ensure_start();
                    let p = Point::new(n(2), n(3));
                    self.path.curve_to(Point::new(n(0), n(1)), p, p);
                    self.cur = Some(p);
                }
                "h" if self.cur.is_some() => {
                    self.path.close_path();
                    self.cur = self.start;
                }
                "re" => {
                    let (x, y, w, h) = (n(0), n(1), n(2), n(3));
                    self.path.move_to((x, y));
                    self.path.line_to((x + w, y));
                    self.path.line_to((x + w, y + h));
                    self.path.line_to((x, y + h));
                    self.path.close_path();
                    self.cur = Some(Point::new(x, y));
                    self.start = self.cur;
                }
                // Painting.
                "S" => self.paint(false, None, true, depth),
                "s" => {
                    self.close_subpath();
                    self.paint(false, None, true, depth)
                }
                "f" | "F" => self.paint(true, Some(FillRule::NonZero), false, depth),
                "f*" => self.paint(true, Some(FillRule::EvenOdd), false, depth),
                "B" => self.paint(true, Some(FillRule::NonZero), true, depth),
                "B*" => self.paint(true, Some(FillRule::EvenOdd), true, depth),
                "b" => {
                    self.close_subpath();
                    self.paint(true, Some(FillRule::NonZero), true, depth)
                }
                "b*" => {
                    self.close_subpath();
                    self.paint(true, Some(FillRule::EvenOdd), true, depth)
                }
                "n" => self.paint(false, None, false, depth),
                "W" => self.pending_clip = Some(FillRule::NonZero),
                "W*" => self.pending_clip = Some(FillRule::EvenOdd),
                // Colour (ignored inside uncoloured patterns and `d1` glyphs).
                "g" | "G" | "rg" | "RG" | "k" | "K" | "cs" | "CS" | "sc" | "scn" | "SC" | "SCN" if locked => {}
                "g" => self.gs.fill_cs = Cs::Gray,
                "G" => self.gs.stroke_cs = Cs::Gray,
                "rg" => self.gs.fill_cs = Cs::Rgb,
                "RG" => self.gs.stroke_cs = Cs::Rgb,
                "k" => self.gs.fill_cs = Cs::Cmyk,
                "K" => self.gs.stroke_cs = Cs::Cmyk,
                "cs" | "CS" => {
                    let cs = args.first().map(|a| color_space(self.file, a, Some(res))).unwrap_or(Cs::Gray);
                    let init = Fill::Color(cs.to_rgb(&cs.initial()));
                    if op == "cs" {
                        self.gs.fill_cs = cs;
                        self.gs.fill = init;
                    } else {
                        self.gs.stroke_cs = cs;
                        self.gs.stroke = init;
                    }
                }
                "sc" | "scn" | "SC" | "SCN" => {
                    let fill = op.starts_with('s');
                    let cs = if fill { self.gs.fill_cs.clone() } else { self.gs.stroke_cs.clone() };
                    let v = if let (Cs::Pattern, Some(name)) = (&cs, args.last().and_then(Obj::name)) {
                        self.pattern(res, name, pattern_base, &nums())
                    } else {
                        Fill::Color(cs.to_rgb(&nums()))
                    };
                    if fill {
                        self.gs.fill = v;
                    } else {
                        self.gs.stroke = v;
                    }
                }
                "sh" => {
                    if let Some(name) = args.first().and_then(Obj::name) {
                        self.shade(res, name);
                    }
                }
                "Do" => {
                    if let Some(name) = args.first().and_then(Obj::name) {
                        self.xobject(res, name, depth);
                    }
                }
                // Text objects (§9.4).
                "BT" => {
                    self.tm = Affine::IDENTITY;
                    self.tlm = Affine::IDENTITY;
                    self.text_clip = None;
                }
                "ET" => {
                    if let Some(clip) = self.text_clip.take() {
                        self.b.clip(clip, FillRule::NonZero);
                    }
                }
                "Tc" => self.gs.char_spacing = n(0),
                "Tw" => self.gs.word_spacing = n(0),
                "Tz" => self.gs.h_scale = n(0) / 100.0,
                "TL" => self.gs.leading = n(0),
                "Ts" => self.gs.rise = n(0),
                "Tr" => self.gs.render_mode = (n(0) as i64).clamp(0, 7) as u8,
                "Tf" => {
                    self.gs.font = args.first().and_then(Obj::name).map(str::to_string).and_then(|name| self.font(res, &name));
                    self.gs.font_size = n(1);
                }
                "Td" => self.text_move(n(0), n(1)),
                "TD" => {
                    self.gs.leading = -n(1);
                    self.text_move(n(0), n(1));
                }
                "Tm" => {
                    self.tm = affine(&nums());
                    self.tlm = self.tm;
                }
                "T*" => self.text_move(0.0, -self.gs.leading),
                "Tj" | "'" | "\"" => {
                    if op == "\"" {
                        self.gs.word_spacing = n(0);
                        self.gs.char_spacing = n(1);
                    }
                    if op != "Tj" {
                        self.text_move(0.0, -self.gs.leading);
                    }
                    if let Some(Obj::Str(s)) = args.iter().rev().find(|a| matches!(a, Obj::Str(_))) {
                        let s = s.clone();
                        self.show(&[TextItem::Str(s)], res, pattern_base, depth);
                    }
                }
                "TJ" => {
                    if let Some(Obj::Array(a)) = args.first() {
                        let items: Vec<TextItem> = a
                            .iter()
                            .filter_map(|x| match x {
                                Obj::Str(s) => Some(TextItem::Str(s.clone())),
                                Obj::Num(v) => Some(TextItem::Adjust(*v)),
                                _ => None,
                            })
                            .collect();
                        self.show(&items, res, pattern_base, depth);
                    }
                }
                "d1" => self.gs.lock_color = true,
                "BI" => self.inline_image(&mut lx, res),
                "BMC" => self.marked.push(None),
                "BDC" => {
                    let layer = match (args.first().and_then(Obj::name), args.get(1)) {
                        (Some("OC"), Some(Obj::Name(prop))) => self
                            .file
                            .get_dict(res, "Properties")
                            .and_then(|p| self.file.get_dict(p, prop))
                            .map(|ocg| self.ocg_name(ocg).unwrap_or_else(|| prop.clone())),
                        (Some("OC"), Some(Obj::Dict(ocg))) => self.ocg_name(ocg),
                        _ => None,
                    };
                    match layer {
                        Some(name) => {
                            self.marked.push(Some(self.b.depth()));
                            self.b.open(&name, true);
                        }
                        None => self.marked.push(None),
                    }
                }
                "EMC" => {
                    if let Some(Some(d)) = self.marked.pop() {
                        self.close_to(d);
                    }
                }
                _ => {}
            }
            // Colour operands set by the device-space shorthands.
            if !locked {
                match op.as_str() {
                    "g" | "rg" | "k" => self.gs.fill = Fill::Color(self.gs.fill_cs.to_rgb(&nums())),
                    "G" | "RG" | "K" => self.gs.stroke = Fill::Color(self.gs.stroke_cs.to_rgb(&nums())),
                    _ => {}
                }
            }
            args.clear();
        }
    }

    /// An inline image: `BI key value … ID data EI` (the lexer is just past `BI`).
    fn inline_image(&mut self, lx: &mut Lexer, res: &Dict) {
        let mut d = Dict::new();
        let mut key: Option<String> = None;
        while let Some(o) = lx.next() {
            match o {
                Obj::Op(ref x) if x == "ID" => break,
                Obj::Name(k) if key.is_none() => key = Some(k),
                v => {
                    if let Some(k) = key.take() {
                        d.insert(k, v);
                    }
                }
            }
        }
        let d = crate::image::expand_inline(&d);
        let data = lx.data;
        let start = (lx.pos + 1).min(data.len());
        let is_ei = |p: usize| {
            p >= 1
                && p + 1 < data.len()
                && crate::object::is_white(data[p - 1])
                && data[p] == b'E'
                && data[p + 1] == b'I'
                && (p + 2 == data.len() || crate::object::is_white(data[p + 2]))
        };
        let end = match crate::image::inline_len(self.file, &d, Some(res)) {
            Some(len) => (start + len).min(data.len()),
            None => {
                let mut p = start;
                while p + 1 < data.len() && !is_ei(p) {
                    p += 1;
                }
                // Data ends before the whitespace preceding `EI`.
                p.saturating_sub(1).max(start).min(data.len())
            }
        };
        let raw = data[start..end].to_vec();
        let mut p = end;
        while p + 1 < data.len() && !(data[p] == b'E' && data[p + 1] == b'I') {
            p += 1;
        }
        lx.pos = (p + 2).min(data.len());
        self.image(&d, &raw, res);
    }

    fn ocg_name(&self, ocg: &Dict) -> Option<String> {
        // An optional-content membership dictionary names its groups in /OCGs.
        let d = match self.file.get(ocg, "OCGs") {
            Some(Obj::Dict(g)) => g,
            Some(Obj::Array(a)) => a.first().map(|x| self.file.resolve(x)).and_then(Obj::dict).unwrap_or(ocg),
            _ => ocg,
        };
        match self.file.get(d, "Name") {
            Some(Obj::Str(s)) => Some(text_string(s)),
            _ => None,
        }
    }

    /// Close the current subpath; nothing to close without a current point.
    fn close_subpath(&mut self) {
        if self.cur.is_some() {
            self.path.close_path();
        }
    }

    fn ensure_start(&mut self) {
        if self.cur.is_none() {
            self.path.move_to((0.0, 0.0));
            self.cur = Some(Point::ZERO);
            self.start = self.cur;
        }
    }

    fn ext_gstate(&mut self, g: &Dict, res: &Dict, depth: usize) {
        let f = self.file;
        if let Some(v) = f.get_num(g, "LW") {
            self.gs.lw = v;
        }
        if let Some(v) = f.get_num(g, "LC") {
            self.gs.cap = [Cap::Butt, Cap::Round, Cap::Square][(v as usize).min(2)];
        }
        if let Some(v) = f.get_num(g, "LJ") {
            self.gs.join = [Join::Miter, Join::Round, Join::Bevel][(v as usize).min(2)];
        }
        if let Some(v) = f.get_num(g, "ML") {
            self.gs.miter = v;
        }
        if let Some(v) = f.get_num(g, "CA") {
            self.gs.stroke_alpha = v.clamp(0.0, 1.0);
        }
        if let Some(v) = f.get_num(g, "ca") {
            self.gs.fill_alpha = v.clamp(0.0, 1.0);
        }
        if let Some(Obj::Array(d)) = f.get(g, "D") {
            let arr = d.first().map(|a| f.nums(a)).unwrap_or_default();
            let ph = d.get(1).and_then(|p| f.resolve(p).num()).unwrap_or(0.0);
            self.gs.dash = (!arr.is_empty() && arr.iter().any(|v| *v > 0.0)).then_some((arr, ph));
        }
        if let Some(Obj::Array(fa)) = f.get(g, "Font")
            && let (Some(Obj::Dict(fd)), Some(size)) = (fa.first().map(|x| f.resolve(x)), fa.get(1).and_then(|x| f.resolve(x).num()))
        {
            let mut sk = vec![];
            self.gs.font = Some(Rc::new(Font::load(f, fd, &mut sk)));
            self.gs.font_size = size;
            for s in sk {
                self.b.skip(&s);
            }
        }
        // Blend mode: a group composited with it, open until the mode changes or `Q`.
        let bm = match f.get(g, "BM") {
            Some(Obj::Name(n)) => Some(n.clone()),
            Some(Obj::Array(a)) => a.first().and_then(|x| f.resolve(x).name()).map(str::to_string),
            _ => None,
        };
        if let Some(bm) = bm {
            let mode = BlendMode::from_pdf_name(&bm);
            if let Some(d) = self.gs.blend_depth {
                self.close_to(d);
            }
            if mode != BlendMode::Normal {
                self.gs.blend_depth = Some(self.b.depth());
                let mut grp = Group::new(&bm);
                grp.blend = mode;
                self.b.open_group(grp);
            }
        }
        // Soft mask: the mask group drawn now, applied to what is painted while it is set.
        match f.get(g, "SMask") {
            None => {}
            Some(Obj::Name(n)) if n == "None" => {
                if let Some(d) = self.gs.mask_depth {
                    self.close_to(d);
                }
            }
            Some(sm) => {
                let Some(sm) = sm.dict().cloned() else { return };
                let luminosity = f.get(&sm, "S").and_then(Obj::name) != Some("Alpha");
                let Some(Obj::Stream(gd, graw)) = f.get(&sm, "G").cloned() else { return };
                // Backdrop: in the group's colour space (black by default).
                let bc = f.get(&sm, "BC").map(|b| f.nums(b)).unwrap_or_default();
                let gcs = f.get_dict(&gd, "Group").and_then(|gr| f.get(gr, "CS")).map(|c| color_space(f, c, Some(res))).unwrap_or(Cs::Rgb);
                let backdrop = if bc.is_empty() { [0.0; 3] } else { gcs.to_rgb(&bc) };
                let saved_b = std::mem::replace(&mut self.b, Builder::new());
                let ctm = self.gs.ctm;
                let saved_gs = std::mem::replace(&mut self.gs, GState { ctm, ..GState::default() });
                let saved_stack = std::mem::take(&mut self.saved);
                self.form(&gd, &graw, res, depth);
                let mask_b = std::mem::replace(&mut self.b, saved_b);
                self.gs = saved_gs;
                self.saved = saved_stack;
                let (nodes, _, sk) = mask_b.finish();
                for s in sk {
                    self.b.skip(&s);
                }
                if let Some(d) = self.gs.mask_depth {
                    self.close_to(d);
                }
                let mut content = Group::new("Soft Mask");
                content.children = nodes;
                let mut grp = Group::new("Soft Mask Group");
                grp.mask = Some(Box::new(SoftMask { content, luminosity, backdrop }));
                self.gs.mask_depth = Some(self.b.depth());
                self.b.open_group(grp);
            }
        }
    }

    fn paint_of(&mut self, f: &Fill) -> Option<Paint> {
        match f {
            Fill::Color(c) => Some(Paint::Color(*c)),
            Fill::Paint(p) => Some(p.clone()),
            Fill::Tiling(t) => {
                self.b.skip("tiling pattern stroke");
                Some(Paint::Color(t.uncolored.unwrap_or([0.5; 3])))
            }
            Fill::Shading(_) => {
                self.b.skip("shading pattern stroke");
                Some(Paint::Color([0.5; 3]))
            }
            Fill::None => None,
        }
    }

    fn paint(&mut self, fill: bool, rule: Option<FillRule>, stroke: bool, depth: usize) {
        let path = std::mem::take(&mut self.path);
        self.cur = None;
        self.start = None;
        self.paint_path(path.clone(), fill, rule.unwrap_or(FillRule::NonZero), stroke, "Path", depth);
        if let Some(r) = self.pending_clip.take() {
            self.b.clip(self.gs.ctm * path, r);
        }
    }

    /// Fill and / or stroke `path` (user space) with the current graphics state.
    fn paint_path(&mut self, path: BezPath, fill: bool, rule: FillRule, stroke: bool, name: &str, depth: usize) {
        if path.elements().is_empty() {
            return;
        }
        let ctm = self.gs.ctm;
        let mut fill_paint = None;
        if fill {
            match self.gs.fill.clone() {
                Fill::Tiling(t) => {
                    let page = ctm * path.clone();
                    let d = self.b.depth();
                    let area = page.bounding_box();
                    self.b.clip(page, rule);
                    self.tile(&t, area, depth);
                    self.close_to(d);
                }
                Fill::Shading(sf) => {
                    let page = ctm * path.clone();
                    let d = self.b.depth();
                    let mut sk = vec![];
                    let img = crate::shading::shading_image(self.file, &sf.sh, sf.space, self.page_box, &mut sk);
                    for s in sk {
                        self.b.skip(&s);
                    }
                    if let Some(mut img) = img {
                        img.opacity = self.gs.fill_alpha;
                        self.b.clip(page, rule);
                        self.b.push(Node::Image(img));
                        self.close_to(d);
                    }
                }
                f => fill_paint = self.paint_of(&f).map(|p| (p, self.gs.fill_alpha, rule)),
            }
        }
        let stroke = if stroke {
            let sf = self.gs.stroke.clone();
            self.paint_of(&sf).map(|p| {
                // Width 0: the thinnest line the device can draw (one pixel).
                let s = { (ctm.determinant().abs()).sqrt() };
                let width = if self.gs.lw <= 0.0 { if s > 0.0 { 1.0 / s } else { 1.0 } } else { self.gs.lw };
                Stroke {
                    paint: p,
                    opacity: self.gs.stroke_alpha,
                    width,
                    cap: self.gs.cap,
                    join: self.gs.join,
                    miter: self.gs.miter.max(1.0),
                    dash: self.gs.dash.clone(),
                }
            })
        } else {
            None
        };
        if fill_paint.is_some() || stroke.is_some() {
            self.b.fill_stroke_named(name, path, ctm, fill_paint, stroke);
        }
    }

    /// Draw a tiling pattern's cells over `area` (page space; the caller clips).
    fn tile(&mut self, t: &Tiling, area: kurbo::Rect, depth: usize) {
        if depth > 8 {
            return;
        }
        // The cell, drawn once in pattern space.
        let saved_b = std::mem::replace(&mut self.b, Builder::new());
        let saved_gs = std::mem::take(&mut self.gs);
        let saved_stack = std::mem::take(&mut self.saved);
        let saved_path = std::mem::take(&mut self.path);
        if let Some(c) = t.uncolored {
            self.gs.fill = Fill::Color(c);
            self.gs.stroke = Fill::Color(c);
            self.gs.lock_color = true;
        }
        self.run(&t.content, &t.res, Affine::IDENTITY, depth + 1);
        let cell_b = std::mem::replace(&mut self.b, saved_b);
        self.gs = saved_gs;
        self.saved = saved_stack;
        self.path = saved_path;
        let (cell, _, sk) = cell_b.finish();
        for s in sk {
            self.b.skip(&s);
        }
        if cell.is_empty() {
            return;
        }
        let (xs, ys) = (t.xstep.abs().max(1e-6), t.ystep.abs().max(1e-6));
        let inv = t.space.inverse();
        let corners = [(area.x0, area.y0), (area.x1, area.y0), (area.x1, area.y1), (area.x0, area.y1)].map(|(x, y)| inv * Point::new(x, y));
        let (mut lo, mut hi) = (Point::new(f64::MAX, f64::MAX), Point::new(f64::MIN, f64::MIN));
        for c in corners {
            lo = Point::new(lo.x.min(c.x), lo.y.min(c.y));
            hi = Point::new(hi.x.max(c.x), hi.y.max(c.y));
        }
        let [bx0, by0, bx1, by1] = t.bbox;
        let i0 = ((lo.x - bx1) / xs).floor();
        let i1 = ((hi.x - bx0) / xs).ceil();
        let j0 = ((lo.y - by1) / ys).floor();
        let j1 = ((hi.y - by0) / ys).ceil();
        if !(i0.is_finite() && i1.is_finite() && j0.is_finite() && j1.is_finite()) || (i1 - i0 + 1.0) * (j1 - j0 + 1.0) > MAX_TILES as f64 {
            self.b.skip("tiling pattern (too many tiles)");
            return;
        }
        let cell_clip = kurbo::Rect::new(bx0, by0, bx1, by1).to_path(0.1);
        let mut tiles = Group::new("Pattern");
        for j in j0 as i64..=j1 as i64 {
            for i in i0 as i64..=i1 as i64 {
                let mut g = Group::new("Tile");
                g.transform = t.space * Affine::translate((i as f64 * xs, j as f64 * ys));
                g.clip.push((cell_clip.clone(), FillRule::NonZero));
                g.children = cell.clone();
                tiles.children.push(Node::Group(g));
            }
        }
        self.b.push(Node::Group(tiles));
    }

    /// A shading dictionary → gradient paint in shading space.
    fn shading_gradient(&mut self, sh: &Dict) -> Option<Gradient> {
        let f = self.file;
        let ty = f.get_num(sh, "ShadingType").unwrap_or(0.0) as i32;
        let cs = f.get(sh, "ColorSpace").map(|c| color_space(f, c, None)).unwrap_or(Cs::Rgb);
        let coords = f.get(sh, "Coords").map(|c| f.nums(c)).unwrap_or_default();
        let func = f.get(sh, "Function").map(|x| Func::read(f, x)).unwrap_or(Func::Unsupported);
        let dom = f.get(sh, "Domain").map(|d| f.nums(d)).filter(|d| d.len() == 2).unwrap_or_else(|| vec![0.0, 1.0]);
        let kind = match (ty, coords.len()) {
            (2, 4) => GradientKind::Linear { x1: coords[0], y1: coords[1], x2: coords[2], y2: coords[3] },
            (3, 6) => GradientKind::Radial { cx: coords[3], cy: coords[4], r: coords[5], fx: coords[0], fy: coords[1] },
            _ => {
                self.b.skip(&format!("shading type {ty}"));
                return None;
            }
        };
        if func == Func::Unsupported {
            self.b.skip("shading function");
        }
        // Sample the function: evenly, plus exactly at stitching bounds.
        let mut ts: Vec<f64> = (0..=24).map(|i| i as f64 / 24.0).collect();
        for b in func.breakpoints() {
            if dom[1] > dom[0] {
                ts.push(((b - dom[0]) / (dom[1] - dom[0])).clamp(0.0, 1.0));
            }
        }
        ts.sort_by(f64::total_cmp);
        ts.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        let stops = ts
            .into_iter()
            .map(|t| {
                let c = cs.to_rgb(&func.eval(&[dom[0] + t * (dom[1] - dom[0])]));
                (t, [c[0], c[1], c[2], 1.0])
            })
            .collect();
        Some(Gradient { kind, stops, bbox_units: false, transform: Affine::IDENTITY, spread: Spread::Pad })
    }

    fn pattern(&mut self, res: &Dict, name: &str, base: Affine, comps: &[f64]) -> Fill {
        let f = self.file;
        let Some(po) = f.get_dict(res, "Pattern").and_then(|pp| pp.get(name)).map(|x| f.resolve(x)) else { return Fill::None };
        let Some(p) = po.dict() else { return Fill::None };
        let m = f.get(p, "Matrix").map(|m| affine(&f.nums(m))).unwrap_or(Affine::IDENTITY);
        match f.get_num(p, "PatternType").unwrap_or(0.0) as i32 {
            2 => {
                let Some(sh_obj) = f.get(p, "Shading") else { return Fill::None };
                let Some(sh) = sh_obj.dict() else { return Fill::None };
                if !matches!(f.get_num(sh, "ShadingType").unwrap_or(0.0) as i32, 2 | 3) {
                    return Fill::Shading(Rc::new(ShadingFill { sh: sh_obj.clone(), space: base * m }));
                }
                let sh = sh.clone();
                match self.shading_gradient(&sh) {
                    // Gradient space → shape user space (shapes carry the CTM).
                    Some(mut g) => {
                        g.transform = self.gs.ctm.inverse() * base * m;
                        Fill::Paint(Paint::Gradient(g))
                    }
                    None => Fill::None,
                }
            }
            1 => {
                let Obj::Stream(d, raw) = po else { return Fill::None };
                let Some(content) = decode_stream(f, d, raw) else { return Fill::None };
                let bb = f.get(d, "BBox").map(|b| f.nums(b)).filter(|b| b.len() == 4).unwrap_or_else(|| vec![0.0, 0.0, 1.0, 1.0]);
                let uncolored = (f.get_num(d, "PaintType").unwrap_or(1.0) as i32 == 2).then(|| match comps.len() {
                    1 => Cs::Gray.to_rgb(comps),
                    4 => Cs::Cmyk.to_rgb(comps),
                    3 => Cs::Rgb.to_rgb(comps),
                    _ => [0.0; 3],
                });
                Fill::Tiling(Rc::new(Tiling {
                    space: base * m,
                    bbox: [bb[0].min(bb[2]), bb[1].min(bb[3]), bb[0].max(bb[2]), bb[1].max(bb[3])],
                    xstep: f.get_num(d, "XStep").unwrap_or(bb[2] - bb[0]),
                    ystep: f.get_num(d, "YStep").unwrap_or(bb[3] - bb[1]),
                    content,
                    res: f.get_dict(d, "Resources").cloned().unwrap_or_else(|| res.clone()),
                    uncolored,
                }))
            }
            _ => Fill::None,
        }
    }

    fn shade(&mut self, res: &Dict, name: &str) {
        let f = self.file;
        let Some(sh_obj) = f.get_dict(res, "Shading").and_then(|s| f.get(s, name)) else { return };
        let Some(sh) = sh_obj.dict().cloned() else { return };
        if !matches!(f.get_num(&sh, "ShadingType").unwrap_or(0.0) as i32, 2 | 3) {
            // Function-based and mesh shadings paint only their own extent.
            let mut sk = vec![];
            let img = crate::shading::shading_image(f, sh_obj, self.gs.ctm, self.page_box, &mut sk);
            for s in sk {
                self.b.skip(&s);
            }
            if let Some(mut img) = img {
                img.opacity = self.gs.fill_alpha;
                self.b.push(Node::Image(img));
            }
            return;
        }
        let Some(g) = self.shading_gradient(&sh) else { return };
        // Fill everything (the current clip limits it): the page box in user space.
        let inv = self.gs.ctm.inverse();
        let b = self.page_box;
        let mut path = BezPath::new();
        for (i, (x, y)) in [(b[0], b[1]), (b[2], b[1]), (b[2], b[3]), (b[0], b[3])].into_iter().enumerate() {
            let p = inv * Point::new(x, y);
            if i == 0 {
                path.move_to(p);
            } else {
                path.line_to(p);
            }
        }
        path.close_path();
        self.b.fill_stroke(path, self.gs.ctm, Some((Paint::Gradient(g), self.gs.fill_alpha, FillRule::NonZero)), None);
    }

    /// Run a form XObject (matrix, bounding-box clip, its own resources) in a saved state.
    fn form(&mut self, d: &Dict, raw: &[u8], res: &Dict, depth: usize) {
        if depth >= 12 {
            return;
        }
        let f = self.file;
        let Some(content) = decode_stream(f, d, raw) else { return };
        let m = f.get(d, "Matrix").map(|m| affine(&f.nums(m))).unwrap_or(Affine::IDENTITY);
        let form_res = f.get_dict(d, "Resources").cloned().unwrap_or_else(|| res.clone());
        self.saved.push((self.gs.clone(), self.b.depth()));
        // A transparency group XObject (§11.6.6) composites as one object: the current alpha
        // (and the blend-mode and soft-mask groups already open around it) apply to its
        // result, and its contents start from the initial alpha.
        let bb = f.get(d, "BBox").map(|b| f.nums(b)).unwrap_or_default();
        let bbox_path = |ctm: Affine| (bb.len() == 4).then(|| ctm * m * kurbo::Rect::new(bb[0], bb[1], bb[2], bb[3]).to_path(0.1));
        let mut clipped = false;
        if let Some(tg) = f.get_dict(d, "Group").filter(|g| f.get(g, "S").and_then(Obj::name) == Some("Transparency")) {
            let mut grp = Group::new("Transparency Group");
            // The bounding box clips the group itself (its children stay its direct children,
            // as knockout needs).
            if let Some(p) = bbox_path(self.gs.ctm) {
                grp.clip.push((p, FillRule::NonZero));
                clipped = true;
            }
            grp.opacity = self.gs.fill_alpha;
            grp.isolated = matches!(f.get(tg, "I"), Some(Obj::Bool(true)));
            grp.knockout = matches!(f.get(tg, "K"), Some(Obj::Bool(true)));
            self.b.open_group(grp);
            self.gs.fill_alpha = 1.0;
            self.gs.stroke_alpha = 1.0;
            self.gs.blend_depth = None;
            self.gs.mask_depth = None;
        }
        if !clipped && let Some(p) = bbox_path(self.gs.ctm) {
            self.b.clip(p, FillRule::NonZero);
        }
        self.gs.ctm *= m;
        let base = self.gs.ctm;
        let path = std::mem::take(&mut self.path);
        let (tm, tlm) = (self.tm, self.tlm);
        self.run(&content, &form_res, base, depth + 1);
        self.path = path;
        self.tm = tm;
        self.tlm = tlm;
        if let Some((g, dd)) = self.saved.pop() {
            self.gs = g;
            self.close_to(dd);
        }
    }

    fn xobject(&mut self, res: &Dict, name: &str, depth: usize) {
        let f = self.file;
        let Some(x) = f.get_dict(res, "XObject").and_then(|xs| xs.get(name)).map(|x| f.resolve(x)) else { return };
        let Obj::Stream(d, raw) = x else { return };
        match f.get(d, "Subtype").and_then(Obj::name) {
            Some("Form") => self.form(d, raw, res, depth),
            Some("Image") => self.image(d, raw, res),
            _ => {}
        }
    }

    /// Paint an image: the unit square of user space, pixel (0, 0) at its top-left corner.
    fn image(&mut self, d: &Dict, raw: &[u8], res: &Dict) {
        let fill = match &self.gs.fill {
            Fill::Color(c) => *c,
            _ => [0.0; 3],
        };
        match crate::image::decode(self.file, d, raw, Some(res), fill) {
            Ok(img) => {
                let (w, h) = (img.width as f64, img.height as f64);
                let transform = self.gs.ctm * Affine::new([1.0 / w, 0.0, 0.0, -1.0 / h, 0.0, 1.0]);
                self.b.push(Node::Image(SvgImage {
                    name: "Image".into(),
                    transform,
                    width: img.width,
                    height: img.height,
                    rgba: std::sync::Arc::new(img.rgba),
                    opacity: self.gs.fill_alpha,
                }));
            }
            Err(what) => self.b.skip(&what),
        }
    }

    /// The font named `name` in the resources (cached by object).
    fn font(&mut self, res: &Dict, name: &str) -> Option<Rc<Font>> {
        let f = self.file;
        let entry = f.get_dict(res, "Font")?.get(name)?;
        let key = match entry {
            Obj::Ref(n, g) => Some(format!("{n} {g}")),
            _ => None,
        };
        if let Some(k) = &key
            && let Some(font) = self.fonts.get(k)
        {
            return Some(font.clone());
        }
        let d = f.resolve(entry).dict()?;
        let mut sk = vec![];
        let font = Rc::new(Font::load(f, d, &mut sk));
        for s in sk {
            self.b.skip(&s);
        }
        if let Some(k) = key {
            self.fonts.insert(k, font.clone());
        }
        Some(font)
    }

    fn text_move(&mut self, tx: f64, ty: f64) {
        self.tlm *= Affine::translate((tx, ty));
        self.tm = self.tlm;
    }

    /// Show text (`Tj`, `TJ`, `'`, `"`): glyph outlines placed by the text matrix, painted by
    /// the text render mode as one compound shape.
    fn show(&mut self, items: &[TextItem], res: &Dict, pattern_base: Affine, depth: usize) {
        let Some(font) = self.gs.font.clone() else {
            self.b.skip("text (no font)");
            return;
        };
        let (fs, th, rise) = (self.gs.font_size, self.gs.h_scale, self.gs.rise);
        let mut path = BezPath::new();
        let mut text = String::new();
        let mut missing = false;
        for it in items {
            match it {
                TextItem::Adjust(v) => {
                    let d = -v / 1000.0 * fs;
                    self.tm *= if font.vertical { Affine::translate((0.0, d)) } else { Affine::translate((d * th, 0.0)) };
                }
                TextItem::Str(s) => {
                    for (code, nbytes) in font.codes(s) {
                        let w0 = font.width(code);
                        let trm = self.tm * Affine::new([fs * th, 0.0, 0.0, fs, 0.0, rise]);
                        let (w1y, vx, vy) = if font.vertical { font.vmetrics(code) } else { (0.0, 0.0, 0.0) };
                        let trm = if font.vertical { trm * Affine::translate((-vx, -vy)) } else { trm };
                        if let Some((proc_, t3res)) = font.type3_proc(code) {
                            let proc_ = proc_.to_vec();
                            let r = t3res.cloned().unwrap_or_else(|| res.clone());
                            self.type3_glyph(&proc_, &r, trm * font.matrix, pattern_base, depth);
                        } else if let Some(g) = font.glyph(code) {
                            path.extend(trm * (*g).clone());
                        } else if code != 32 {
                            missing = true;
                        }
                        text.push_str(&font.unicode(code));
                        let tw = if nbytes == 1 && code == 32 { self.gs.word_spacing } else { 0.0 };
                        if font.vertical {
                            self.tm *= Affine::translate((0.0, w1y * fs - self.gs.char_spacing - tw));
                        } else {
                            self.tm *= Affine::translate(((w0 * fs + self.gs.char_spacing + tw) * th, 0.0));
                        }
                    }
                }
            }
        }
        if missing && !matches!(font.program, crate::font::Program::Type3 { .. }) {
            self.b.skip(&format!("glyphs missing from font {}", font.base_name));
        }
        if path.elements().is_empty() {
            return;
        }
        let mode = self.gs.render_mode;
        let fill = matches!(mode, 0 | 2 | 4 | 6);
        let stroke = matches!(mode, 1 | 2 | 5 | 6);
        let label: String = text.chars().filter(|c| !c.is_control()).take(40).collect();
        let name = if label.trim().is_empty() { "Text".to_string() } else { format!("Text: {}", label.trim()) };
        if mode >= 4 {
            let clip = self.text_clip.get_or_insert_with(BezPath::new);
            clip.extend(self.gs.ctm * path.clone());
        }
        if fill || stroke {
            self.paint_path(path, fill, FillRule::NonZero, stroke, &name, depth);
        }
    }

    /// Run a Type 3 glyph procedure with `m` (glyph space → user space).
    fn type3_glyph(&mut self, content: &[u8], res: &Dict, m: Affine, pattern_base: Affine, depth: usize) {
        if depth >= 8 {
            return;
        }
        self.saved.push((self.gs.clone(), self.b.depth()));
        self.gs.ctm *= m;
        let path = std::mem::take(&mut self.path);
        let (tm, tlm) = (self.tm, self.tlm);
        self.run(content, res, pattern_base, depth + 1);
        self.path = path;
        self.tm = tm;
        self.tlm = tlm;
        if let Some((g, d)) = self.saved.pop() {
            self.gs = g;
            self.close_to(d);
        }
    }
}
