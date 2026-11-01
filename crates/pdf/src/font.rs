//! PDF fonts (ISO 32000-1 §9.5–9.10): simple fonts (Type 1, TrueType, Type 3) and composite
//! (Type 0) fonts with their encodings, widths and embedded programs, turned into glyph
//! outlines in text space.
//!
//! Embedded programs: TrueType (`FontFile2`, through skrifa), CFF (`FontFile3` `/Type1C`,
//! `/CIDFontType0C` and OpenType, [`crate::cff`]), Type 1 (`FontFile`, [`crate::type1`]) and
//! Type 3 glyph procedures (drawn by the content interpreter). Fonts that are not embedded
//! (the standard 14 and others) are drawn with the bundled OFL fonts: Inter for sans serif,
//! Noto Serif for serif and JetBrains Mono for monospaced families.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use kurbo::{Affine, BezPath};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::raw::TableProvider;
use skrifa::raw::tables::cmap::PlatformId;
use skrifa::{FontRef, GlyphId, MetadataProvider};

use crate::cff::Cff;
use crate::encoding::{Base, name_to_unicode};
use crate::object::{Dict, File, Lexer, Obj, decode_stream};
use crate::type1::Type1;

pub(crate) enum Program {
    TrueType(Arc<Vec<u8>>),
    Cff(Box<Cff>),
    Type1(Box<Type1>),
    /// A bundled stand-in (not embedded).
    Fallback(&'static [u8]),
    /// Glyph procedures: (name → content stream, resources).
    Type3 {
        procs: HashMap<String, Vec<u8>>,
        resources: Option<Dict>,
    },
    None,
}

pub(crate) struct Font {
    /// Two-byte (or CMap-coded) codes and CIDs.
    pub composite: bool,
    pub program: Program,
    /// Glyph space → text space.
    pub matrix: Affine,
    first_char: u32,
    widths: Vec<f64>,
    cid_widths: HashMap<u32, f64>,
    default_width: f64,
    /// Simple fonts: code → glyph name.
    names: Vec<Option<String>>,
    symbolic: bool,
    to_unicode: HashMap<u32, String>,
    cid_to_gid: Option<Vec<u16>>,
    /// Composite fonts: code space ranges (bytes, lo, hi) and code → CID ranges.
    codespace: Vec<(usize, u32, u32)>,
    cid_ranges: Vec<(u32, u32, u32)>,
    pub vertical: bool,
    /// Vertical metrics (`/W2`, in text space): CID → (w1y, vx, vy).
    cid_vmetrics: HashMap<u32, (f64, f64, f64)>,
    /// `/DW2`: default (vy, w1y).
    dw2: (f64, f64),
    cache: RefCell<HashMap<u32, Option<Arc<BezPath>>>>,
    /// Program glyph advances (font units → text space) for fonts without `/Widths`.
    has_widths: bool,
    pub base_name: String,
}

struct Pen(BezPath);

impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to((x as f64, y as f64));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to((x as f64, y as f64));
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.0.quad_to((cx0 as f64, cy0 as f64), (x as f64, y as f64));
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.0.curve_to((cx0 as f64, cy0 as f64), (cx1 as f64, cy1 as f64), (x as f64, y as f64));
    }
    fn close(&mut self) {
        self.0.close_path();
    }
}

/// A bundled stand-in for a font that is not embedded, by its PostScript name.
fn fallback(base: &str) -> &'static [u8] {
    use effectcraft_text::fonts::{INTER_BOLD, INTER_ITALIC, INTER_REGULAR, INTER_SEMIBOLD, JETBRAINS_MONO_REGULAR, NOTO_SERIF_REGULAR};
    let b = base.to_ascii_lowercase();
    if b.contains("courier") || b.contains("mono") || b.contains("consol") {
        return JETBRAINS_MONO_REGULAR;
    }
    if b.contains("times")
        || b.contains("serif") && !b.contains("sans")
        || b.contains("georgia")
        || b.contains("garamond")
        || b.contains("minion")
        || b.contains("roman")
    {
        return NOTO_SERIF_REGULAR;
    }
    if b.contains("bold") || b.contains("black") || b.contains("heavy") {
        return INTER_BOLD;
    }
    if b.contains("semibold") || b.contains("demi") {
        return INTER_SEMIBOLD;
    }
    if b.contains("italic") || b.contains("oblique") {
        return INTER_ITALIC;
    }
    INTER_REGULAR
}

fn sfnt_upem(data: &[u8]) -> f64 {
    FontRef::new(data).ok().and_then(|f| f.head().ok().map(|h| h.units_per_em() as f64)).filter(|u| *u > 0.0).unwrap_or(1000.0)
}

/// A TrueType / OpenType glyph's outline in font units.
fn sfnt_outline(data: &[u8], gid: u16) -> Option<BezPath> {
    let font = FontRef::new(data).ok()?;
    let g = font.outline_glyphs().get(GlyphId::new(gid as u32))?;
    let mut pen = Pen(BezPath::new());
    g.draw(DrawSettings::unhinted(Size::unscaled(), LocationRef::default()), &mut pen).ok()?;
    Some(pen.0)
}

fn sfnt_advance(data: &[u8], gid: u16) -> Option<f64> {
    let font = FontRef::new(data).ok()?;
    font.glyph_metrics(Size::unscaled(), LocationRef::default()).advance_width(GlyphId::new(gid as u32)).map(|a| a as f64)
}

/// A glyph of a TrueType font for a simple-font code (§9.6.6.4): the (3,1) subtable through
/// the glyph's Unicode value, the (3,0) symbol subtable at `0xF000 + code`, the (1,0) Macintosh
/// subtable by code, then `post` glyph names.
fn sfnt_glyph(data: &[u8], code: u8, name: Option<&str>, symbolic: bool) -> Option<u16> {
    let font = FontRef::new(data).ok()?;
    let cmap = font.cmap().ok();
    let sub = |pid: PlatformId, eid: u16| {
        let cmap = cmap.as_ref()?;
        cmap.encoding_records().iter().find(|r| r.platform_id() == pid && r.encoding_id() == eid).and_then(|r| r.subtable(cmap.offset_data()).ok())
    };
    let uni = name.and_then(name_to_unicode);
    let ok = |g: Option<GlyphId>| g.map(|g| g.to_u32() as u16).filter(|g| *g != 0);
    if !symbolic
        && let Some(u) = uni
        && let Some(g) = ok(sub(PlatformId::Windows, 1).and_then(|s| s.map_codepoint(u as u32))).or_else(|| ok(font.charmap().map(u)))
    {
        return Some(g);
    }
    if let Some(s) = sub(PlatformId::Windows, 0) {
        for c in [0xF000 + code as u32, 0xF100 + code as u32, 0xF200 + code as u32, code as u32] {
            if let Some(g) = ok(s.map_codepoint(c)) {
                return Some(g);
            }
        }
    }
    if let Some(g) = ok(sub(PlatformId::Macintosh, 0).and_then(|s| s.map_codepoint(code as u32))) {
        return Some(g);
    }
    if let Some(n) = name
        && let Some((g, _)) = font.glyph_names().iter().find(|(_, gn)| gn.as_str() == n)
    {
        return Some(g.to_u32() as u16);
    }
    if let Some(u) = uni {
        return ok(font.charmap().map(u));
    }
    // Symbolic fonts without a usable cmap: the code as the glyph index.
    (symbolic && code != 0).then_some(code as u16)
}

/// `/Differences [code /name /name … code /name …]`.
fn differences(file: &File, enc: &Dict, names: &mut [Option<String>]) {
    let Some(diffs) = file.get(enc, "Differences").and_then(Obj::array) else { return };
    let mut code = 0usize;
    for d in diffs {
        match file.resolve(d) {
            Obj::Num(n) => code = *n as usize,
            Obj::Name(n) => {
                if let Some(slot) = names.get_mut(code) {
                    *slot = Some(n.clone());
                }
                code += 1;
            }
            _ => {}
        }
    }
}

/// A ToUnicode or encoding CMap: `bfchar` / `bfrange` → strings, `cidchar` / `cidrange` →
/// CIDs, `codespacerange` → code lengths.
#[derive(Default)]
struct CMap {
    codespace: Vec<(usize, u32, u32)>,
    unicode: HashMap<u32, String>,
    cids: Vec<(u32, u32, u32)>,
    /// `/WMode 1`: vertical writing.
    wmode: bool,
}

fn be(b: &[u8]) -> u32 {
    b.iter().fold(0u32, |a, &c| (a << 8) | c as u32)
}

fn utf16(b: &[u8]) -> String {
    let u: Vec<u16> = b.chunks(2).map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)])).collect();
    String::from_utf16_lossy(&u)
}

fn parse_cmap(data: &[u8]) -> CMap {
    let mut m = CMap::default();
    let mut lx = Lexer::new(data, 0);
    let mut args: Vec<Obj> = vec![];
    let mut mode = "";
    while let Some(o) = lx.next() {
        let Obj::Op(op) = o else {
            args.push(o);
            continue;
        };
        match op.as_str() {
            "begincodespacerange" => mode = "cs",
            "beginbfchar" => mode = "bfchar",
            "beginbfrange" => mode = "bfrange",
            "begincidchar" => mode = "cidchar",
            "begincidrange" => mode = "cidrange",
            "endcodespacerange" | "endbfchar" | "endbfrange" | "endcidchar" | "endcidrange" => {
                let a = std::mem::take(&mut args);
                match mode {
                    "cs" => {
                        for p in a.as_chunks::<2>().0 {
                            if let (Obj::Str(lo), Obj::Str(hi)) = (&p[0], &p[1]) {
                                m.codespace.push((lo.len().max(1), be(lo), be(hi)));
                            }
                        }
                    }
                    "bfchar" => {
                        for p in a.as_chunks::<2>().0 {
                            if let (Obj::Str(src), Obj::Str(dst)) = (&p[0], &p[1]) {
                                m.unicode.insert(be(src), utf16(dst));
                            }
                        }
                    }
                    "bfrange" => {
                        for p in a.as_chunks::<3>().0 {
                            let (Obj::Str(lo), Obj::Str(hi)) = (&p[0], &p[1]) else { continue };
                            let (lo, hi) = (be(lo), be(hi));
                            if hi < lo || hi - lo > 65535 {
                                continue;
                            }
                            match &p[2] {
                                Obj::Str(dst) => {
                                    let mut d = dst.clone();
                                    for c in lo..=hi {
                                        m.unicode.insert(c, utf16(&d));
                                        if let Some(last) = d.last_mut() {
                                            *last = last.wrapping_add(1);
                                        }
                                    }
                                }
                                Obj::Array(list) => {
                                    for (k, s) in list.iter().enumerate() {
                                        if let (Obj::Str(s), Some(c)) = (s, lo.checked_add(k as u32)) {
                                            m.unicode.insert(c, utf16(s));
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    "cidchar" => {
                        for p in a.as_chunks::<2>().0 {
                            if let (Obj::Str(src), Some(cid)) = (&p[0], p[1].num()) {
                                let c = be(src);
                                m.cids.push((c, c, cid as u32));
                            }
                        }
                    }
                    "cidrange" => {
                        for p in a.as_chunks::<3>().0 {
                            if let (Obj::Str(lo), Obj::Str(hi), Some(cid)) = (&p[0], &p[1], p[2].num()) {
                                m.cids.push((be(lo), be(hi), cid as u32));
                            }
                        }
                    }
                    _ => {}
                }
                mode = "";
            }
            "def" if mode.is_empty() => {
                if let [.., Obj::Name(k), Obj::Num(v)] = args.as_slice()
                    && k == "WMode"
                {
                    m.wmode = *v == 1.0;
                }
                args.clear();
            }
            _ => {
                if mode.is_empty() {
                    args.clear();
                }
            }
        }
    }
    m
}

impl Font {
    /// Read a font dictionary. `skipped` collects what could not be read.
    fn blank(base_name: &str, composite: bool) -> Font {
        Font {
            composite,
            program: Program::None,
            matrix: Affine::scale(0.001),
            first_char: 0,
            widths: vec![],
            cid_widths: HashMap::new(),
            default_width: 0.0,
            names: vec![None; 256],
            symbolic: false,
            to_unicode: HashMap::new(),
            cid_to_gid: None,
            codespace: vec![],
            cid_ranges: vec![],
            vertical: false,
            cid_vmetrics: HashMap::new(),
            dw2: (0.88, -1.0),
            cache: RefCell::new(HashMap::new()),
            has_widths: false,
            base_name: base_name.to_string(),
        }
    }

    /// A simple font outside a PDF (EPS text): `program` is an embedded font program (CFF when
    /// the flag is set, else Type 1), otherwise the bundled stand-in for `name` is used;
    /// `names` is the encoding (code → glyph name), else the program's built-in encoding or
    /// StandardEncoding. Advances are the program's.
    pub fn standalone(name: &str, program: Option<(&[u8], bool)>, names: Option<Vec<Option<String>>>) -> Font {
        let mut f = Font::blank(name, false);
        f.program = match program {
            Some((data, true)) => Cff::parse(data).map(|c| Program::Cff(Box::new(c))),
            Some((data, false)) => Type1::parse(data).map(|t| Program::Type1(Box::new(t))),
            None => None,
        }
        .unwrap_or_else(|| Program::Fallback(fallback(name)));
        match &f.program {
            Program::Cff(c) => f.matrix = Affine::new(c.font_matrix),
            Program::Type1(t) => f.matrix = t.font_matrix.map(Affine::new).unwrap_or(f.matrix),
            _ => {}
        }
        let symbol = matches!(f.program, Program::Fallback(_)) && (name.contains("Symbol") || name.contains("Dingbats"));
        f.names = match names {
            // Unencoded codes draw .notdef (not the built-in encoding).
            Some(n) => (0..256).map(|c| Some(n.get(c).cloned().flatten().unwrap_or_else(|| ".notdef".into()))).collect(),
            None => match &f.program {
                Program::Type1(t) if !t.encoding.is_empty() => (0..256).map(|c| t.encoding.get(&(c as u8)).cloned()).collect(),
                _ if symbol => vec![None; 256],
                _ => (0..256).map(|c| Base::Standard.name(c as u8)).collect(),
            },
        };
        f
    }

    pub fn load(file: &File, d: &Dict, skipped: &mut Vec<String>) -> Font {
        let subtype = file.get(d, "Subtype").and_then(Obj::name).unwrap_or("Type1").to_string();
        let base_name = file.get(d, "BaseFont").and_then(Obj::name).unwrap_or("").to_string();
        let mut f = Font::blank(&base_name, subtype == "Type0");
        if let Some(Obj::Stream(sd, raw)) = file.get(d, "ToUnicode")
            && let Some(data) = decode_stream(file, sd, raw)
        {
            f.to_unicode = parse_cmap(&data).unicode;
        }
        let (desc_dict, fd) = if f.composite {
            let desc = file
                .get(d, "DescendantFonts")
                .and_then(Obj::array)
                .and_then(|a| a.first())
                .map(|x| file.resolve(x))
                .and_then(Obj::dict)
                .cloned()
                .unwrap_or_default();
            let fd = file.get_dict(&desc, "FontDescriptor").cloned();
            (desc, fd)
        } else {
            (d.clone(), file.get_dict(d, "FontDescriptor").cloned())
        };
        if let Some(fd) = &fd {
            f.symbolic = file.get_num(fd, "Flags").is_some_and(|fl| (fl as u32) & 4 != 0);
        }
        // Widths.
        if f.composite {
            f.load_cid_widths(file, &desc_dict);
            f.load_vertical_metrics(file, &desc_dict);
            f.has_widths = true;
            f.load_cid_encoding(file, d, &desc_dict, skipped);
        } else {
            f.first_char = file.get_num(d, "FirstChar").unwrap_or(0.0) as u32;
            if let Some(w) = file.get(d, "Widths") {
                f.widths = file.nums(w);
                f.has_widths = !f.widths.is_empty();
            }
            if let Some(fd) = &fd {
                f.default_width = file.get_num(fd, "MissingWidth").unwrap_or(0.0);
            }
        }
        f.load_program(file, d, fd.as_ref(), &subtype, skipped);
        f.load_simple_encoding(file, d);
        f
    }

    fn load_cid_widths(&mut self, file: &File, desc_dict: &Dict) {
        self.default_width = file.get_num(desc_dict, "DW").unwrap_or(1000.0) / 1000.0;
        if let Some(w) = file.get(desc_dict, "W").and_then(Obj::array) {
            let mut i = 0;
            while i < w.len() {
                let first = file.resolve(&w[i]).num().unwrap_or(0.0) as u32;
                match w.get(i + 1).map(|x| file.resolve(x)) {
                    Some(Obj::Array(list)) => {
                        for (k, v) in list.iter().enumerate() {
                            if let Some(cid) = first.checked_add(k as u32) {
                                self.cid_widths.insert(cid, file.resolve(v).num().unwrap_or(0.0) / 1000.0);
                            }
                        }
                        i += 2;
                    }
                    Some(Obj::Num(last)) => {
                        let wv = w.get(i + 2).and_then(|x| file.resolve(x).num()).unwrap_or(0.0) / 1000.0;
                        for c in first..=(*last as u32).min(first.saturating_add(65535)) {
                            self.cid_widths.insert(c, wv);
                        }
                        i += 3;
                    }
                    _ => break,
                }
            }
        }
    }

    fn load_vertical_metrics(&mut self, file: &File, desc_dict: &Dict) {
        // Vertical metrics (§9.7.4.3): /DW2 [vy w1y] and /W2 entries
        // `c [w1y vx vy …]` or `cfirst clast w1y vx vy`.
        let dw2 = file.get(desc_dict, "DW2").map(|x| file.nums(x)).unwrap_or_default();
        if dw2.len() == 2 {
            self.dw2 = (dw2[0] / 1000.0, dw2[1] / 1000.0);
        }
        if let Some(w) = file.get(desc_dict, "W2").and_then(Obj::array) {
            let num = |i: usize| w.get(i).and_then(|x| file.resolve(x).num());
            let mut i = 0;
            while i < w.len() {
                let Some(first) = num(i) else { break };
                let first = first.max(0.0) as u32;
                match w.get(i + 1).map(|x| file.resolve(x)) {
                    Some(Obj::Array(list)) => {
                        let v: Vec<f64> = list.iter().filter_map(|x| file.resolve(x).num()).collect();
                        for (k, t) in v.as_chunks::<3>().0.iter().enumerate() {
                            if let Some(cid) = first.checked_add(k as u32) {
                                self.cid_vmetrics.insert(cid, (t[0] / 1000.0, t[1] / 1000.0, t[2] / 1000.0));
                            }
                        }
                        i += 2;
                    }
                    Some(Obj::Num(last)) => {
                        let (Some(w1), Some(vx), Some(vy)) = (num(i + 2), num(i + 3), num(i + 4)) else { break };
                        for c in first..=(last.max(0.0) as u32).min(first.saturating_add(65535)) {
                            self.cid_vmetrics.insert(c, (w1 / 1000.0, vx / 1000.0, vy / 1000.0));
                        }
                        i += 5;
                    }
                    _ => break,
                }
            }
        }
    }

    fn load_cid_encoding(&mut self, file: &File, d: &Dict, desc_dict: &Dict, skipped: &mut Vec<String>) {
        // Encoding CMap.
        match file.get(d, "Encoding") {
            Some(Obj::Name(n)) => {
                self.vertical = n.ends_with("-V");
                if !n.starts_with("Identity") {
                    skipped.push(format!("CMap {n}"));
                }
            }
            Some(Obj::Stream(sd, raw)) => {
                if let Some(data) = decode_stream(file, sd, raw) {
                    let cm = parse_cmap(&data);
                    self.vertical = cm.wmode || file.get_num(sd, "WMode") == Some(1.0);
                    self.codespace = cm.codespace;
                    self.cid_ranges = cm.cids;
                }
            }
            _ => {}
        }
        match file.get(desc_dict, "CIDToGIDMap") {
            Some(Obj::Stream(sd, raw)) => {
                self.cid_to_gid = decode_stream(file, sd, raw).map(|b| b.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes([c[0], c[1]])).collect());
            }
            _ => self.cid_to_gid = None,
        }
    }

    fn load_program(&mut self, file: &File, d: &Dict, fd: Option<&Dict>, subtype: &str, skipped: &mut Vec<String>) {
        // The program.
        let font_file = |key: &str| -> Option<(Dict, Vec<u8>)> {
            let fd = fd?;
            match file.get(fd, key) {
                Some(Obj::Stream(sd, raw)) => Some((sd.clone(), decode_stream(file, sd, raw)?)),
                _ => None,
            }
        };
        if subtype == "Type3" {
            self.matrix = file
                .get(d, "FontMatrix")
                .map(|m| file.nums(m))
                .filter(|m| m.len() == 6)
                .map(|m| Affine::new([m[0], m[1], m[2], m[3], m[4], m[5]]))
                .unwrap_or(Affine::scale(0.001));
            let mut procs = HashMap::new();
            if let Some(cp) = file.get_dict(d, "CharProcs") {
                for (k, v) in cp {
                    if let Obj::Stream(sd, raw) = file.resolve(v)
                        && let Some(data) = decode_stream(file, sd, raw)
                    {
                        procs.insert(k.clone(), data);
                    }
                }
            }
            self.program = Program::Type3 { procs, resources: file.get_dict(d, "Resources").cloned() };
            // Type 3 widths are in glyph space.
            let m = self.matrix.as_coeffs()[0];
            for w in &mut self.widths {
                *w *= m * 1000.0;
            }
        } else if let Some((_, data)) = font_file("FontFile2") {
            self.program = Program::TrueType(Arc::new(data));
        } else if let Some((sd, data)) = font_file("FontFile3") {
            match file.get(&sd, "Subtype").and_then(Obj::name) {
                Some("OpenType") => {
                    // An OpenType wrapper: TrueType outlines through skrifa, CFF through ours.
                    let cff = FontRef::new(&data).ok().and_then(|fr| fr.table_data(skrifa::Tag::new(b"CFF ")).map(|t| t.as_bytes().to_vec()));
                    match cff.and_then(|c| Cff::parse(&c)) {
                        Some(c) => self.program = Program::Cff(Box::new(c)),
                        None => self.program = Program::TrueType(Arc::new(data)),
                    }
                }
                _ => match Cff::parse(&data) {
                    Some(c) => self.program = Program::Cff(Box::new(c)),
                    None => skipped.push("font program (CFF)".into()),
                },
            }
        } else if let Some((_, data)) = font_file("FontFile") {
            match Type1::parse(&data) {
                Some(t) => self.program = Program::Type1(Box::new(t)),
                None => skipped.push("font program (Type 1)".into()),
            }
        } else {
            self.program = Program::Fallback(fallback(&self.base_name));
        }
        if let Program::Cff(c) = &self.program {
            let m = c.font_matrix;
            self.matrix = Affine::new(m);
        }
        if let Program::Type1(t) = &self.program
            && let Some(m) = t.font_matrix
        {
            self.matrix = Affine::new(m);
        }
    }

    fn load_simple_encoding(&mut self, file: &File, d: &Dict) {
        // Simple-font encoding: base encoding, built-in encoding, Differences.
        if !self.composite {
            let enc = file.get(d, "Encoding").map(|e| file.resolve(e));
            let base = match enc {
                Some(Obj::Name(n)) => Base::from_name(n),
                Some(Obj::Dict(e)) => file.get(e, "BaseEncoding").and_then(Obj::name).and_then(Base::from_name),
                _ => None,
            };
            let builtin: Option<Vec<Option<String>>> = match &self.program {
                Program::Type1(t) if !t.encoding.is_empty() => Some((0..256).map(|c| t.encoding.get(&(c as u8)).cloned()).collect()),
                _ => None,
            };
            let fallback_base = if self.symbolic { None } else { Some(Base::Standard) };
            for c in 0..256usize {
                self.names[c] = match (base, &builtin) {
                    (Some(b), _) => b.name(c as u8),
                    (None, Some(bi)) => bi[c].clone(),
                    (None, None) => match &self.program {
                        // TrueType symbolic fonts map codes directly; others use Standard.
                        Program::TrueType(_) if self.symbolic => None,
                        Program::Fallback(_) if self.base_name.contains("Symbol") || self.base_name.contains("Dingbats") => None,
                        _ => fallback_base.or(Some(Base::Standard)).and_then(|b| b.name(c as u8)),
                    },
                };
            }
            if let Some(Obj::Dict(e)) = enc {
                differences(file, e, &mut self.names);
            }
        }
    }

    /// Split a string into (code, byte length) pairs.
    pub fn codes(&self, s: &[u8]) -> Vec<(u32, usize)> {
        if !self.composite {
            return s.iter().map(|&b| (b as u32, 1)).collect();
        }
        let mut out = vec![];
        let mut i = 0;
        while i < s.len() {
            let mut len = 2;
            if !self.codespace.is_empty() {
                len = (1..=4)
                    .find(|&n| {
                        i + n <= s.len() && {
                            let v = be(&s[i..i + n]);
                            self.codespace.iter().any(|(l, lo, hi)| *l == n && v >= *lo && v <= *hi)
                        }
                    })
                    .unwrap_or(1);
            }
            let end = (i + len).min(s.len());
            out.push((be(&s[i..end]), end - i));
            i = end;
        }
        out
    }

    fn cid(&self, code: u32) -> u32 {
        if self.cid_ranges.is_empty() {
            return code;
        }
        self.cid_ranges.iter().find(|(lo, hi, _)| code >= *lo && code <= *hi).and_then(|(lo, _, c)| c.checked_add(code - lo)).unwrap_or(0)
    }

    /// Vertical metrics of a code in text space (before the font size): the vertical
    /// displacement `w1y` (negative: downwards) and the position vector `(vx, vy)` from the
    /// horizontal origin to the vertical origin (§9.7.4.3; defaults from `/DW2`, `vx` half the
    /// horizontal advance).
    pub fn vmetrics(&self, code: u32) -> (f64, f64, f64) {
        match self.cid_vmetrics.get(&self.cid(code)) {
            Some(m) => *m,
            None => (self.dw2.1, self.width(code) / 2.0, self.dw2.0),
        }
    }

    /// The advance of a code in text space (before the font size).
    pub fn width(&self, code: u32) -> f64 {
        if self.composite {
            return self.cid_widths.get(&self.cid(code)).copied().unwrap_or(self.default_width);
        }
        if self.has_widths {
            return match code.checked_sub(self.first_char).and_then(|i| self.widths.get(i as usize)) {
                Some(w) => w / 1000.0,
                None => self.default_width / 1000.0,
            };
        }
        // No /Widths (standard 14 fonts): the program's own advance.
        match &self.program {
            Program::Fallback(data) => {
                self.sfnt_gid(data, code).and_then(|g| sfnt_advance(data, g)).map(|a| a / sfnt_upem(data)).unwrap_or(if code == 32 { 0.25 } else { 0.5 })
            }
            Program::TrueType(data) => self.sfnt_gid(data, code).and_then(|g| sfnt_advance(data, g)).map(|a| a / sfnt_upem(data)).unwrap_or(0.5),
            Program::Type1(t) => self.name(code).and_then(|n| t.outline(n)).map(|(_, w)| self.matrix.as_coeffs()[0] * w).unwrap_or(0.5),
            Program::Cff(c) => {
                self.name(code).and_then(|n| c.gid_by_name(n)).and_then(|g| c.outline(g)).map(|(_, w)| self.matrix.as_coeffs()[0] * w).unwrap_or(0.5)
            }
            _ => 0.5,
        }
    }

    fn name(&self, code: u32) -> Option<&str> {
        self.names.get(code as usize).and_then(|n| n.as_deref())
    }

    fn sfnt_gid(&self, data: &[u8], code: u32) -> Option<u16> {
        if self.composite {
            let cid = self.cid(code);
            return Some(match &self.cid_to_gid {
                Some(map) => *map.get(cid as usize)?,
                None => cid as u16,
            });
        }
        // Stand-ins are looked up by Unicode (ToUnicode first).
        if let Program::Fallback(_) = self.program
            && let Some(u) = self.to_unicode.get(&code).and_then(|s| s.chars().next())
        {
            let font = FontRef::new(data).ok()?;
            return font.charmap().map(u).map(|g| g.to_u32() as u16);
        }
        sfnt_glyph(data, code as u8, self.name(code), self.symbolic && !matches!(self.program, Program::Fallback(_)))
    }

    /// The Type 3 glyph procedure of a code.
    pub fn type3_proc(&self, code: u32) -> Option<(&[u8], Option<&Dict>)> {
        let Program::Type3 { procs, resources } = &self.program else { return None };
        let n = self.name(code)?;
        procs.get(n).map(|p| (p.as_slice(), resources.as_ref()))
    }

    /// A code's outline in text space (font size 1), cached.
    pub fn glyph(&self, code: u32) -> Option<Arc<BezPath>> {
        if let Some(v) = self.cache.borrow().get(&code) {
            return v.clone();
        }
        let path = self.glyph_uncached(code).map(Arc::new);
        self.cache.borrow_mut().insert(code, path.clone());
        path
    }

    fn glyph_uncached(&self, code: u32) -> Option<BezPath> {
        match &self.program {
            Program::TrueType(data) => self.sfnt_path(data, code),
            Program::Fallback(data) => self.sfnt_path(data, code),
            Program::Cff(c) => {
                let g = if self.composite {
                    c.gid_by_cid(self.cid(code) as u16)?
                } else {
                    match self.name(code).and_then(|n| c.gid_by_name(n)) {
                        Some(g) => g,
                        None => Some(c.encoding[(code & 255) as usize]).filter(|g| *g != 0)?,
                    }
                };
                c.outline(g).map(|(p, _)| self.matrix * p)
            }
            Program::Type1(t) => {
                let name = self.name(code).filter(|n| t.has_glyph(n)).or_else(|| t.encoding.get(&(code as u8)).map(String::as_str))?;
                t.outline(name).map(|(p, _)| self.matrix * p)
            }
            Program::Type3 { .. } | Program::None => None,
        }
    }

    fn sfnt_path(&self, data: &[u8], code: u32) -> Option<BezPath> {
        let g = self.sfnt_gid(data, code)?;
        let p = sfnt_outline(data, g)?;
        Some(Affine::scale(1.0 / sfnt_upem(data)) * p)
    }

    /// The text of a code (ToUnicode, else the glyph name), for naming text shapes.
    pub fn unicode(&self, code: u32) -> String {
        if let Some(s) = self.to_unicode.get(&code) {
            return s.clone();
        }
        if self.composite {
            return String::new();
        }
        self.name(code).and_then(name_to_unicode).map(String::from).unwrap_or_default()
    }
}
