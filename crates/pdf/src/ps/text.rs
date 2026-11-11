use super::scanner::Src;
use super::value::{DictRef, Flow, V, array, matrix_of, new_dict};
use super::{Affine, BezPath, FastKey, FillRule, Font, FontKey, Glyphs, HashMap, Interp, NamePtr, Paint, Point, Rc, RefCell};

// ------------------------------------------------------------------ text

/// The exact payload an encoding array keeps: 256 slots and each fresh glyph name's text.
/// `Base::name` returns an owned `String`; measure it temporarily before retaining any names.
pub(super) fn encoding_bytes(base: crate::encoding::Base) -> usize {
    256 * std::mem::size_of::<V>() + (0..=u8::MAX).map(|c| base.name(c).map_or(".notdef".len(), |n| n.len())).sum::<usize>()
}

/// An encoding array of glyph names, constructed only after its full payload is charged.
/// The caller selects StandardEncoding or the legacy WinAnsi approximation for other encodings.
pub(super) fn encoding_array(base: crate::encoding::Base) -> V {
    array((0..=u8::MAX).map(|c| V::Lit(base.name(c).unwrap_or_else(|| ".notdef".into()).into())).collect())
}

/// An encoding entry's glyph name, shared: a name other than `.notdef`, else none.
fn encoded_name(v: &V) -> Option<Rc<str>> {
    match v {
        V::Lit(n) | V::Name(n) if &**n != ".notdef" => Some(n.clone()),
        _ => None,
    }
}

/// The pointer-identity key for a font built from `base`, drawn with the one `glyph` or the first
/// 256 names of `encoding`. It mirrors the [`Glyphs`] the content key uses, so it tells two fonts
/// apart exactly when their names differ by identity (a re-encoding makes fresh, charged names).
fn fast_key(base: &Rc<str>, glyph: Option<&Rc<str>>, encoding: Option<&V>) -> FastKey {
    let base = NamePtr(Some(base.clone()));
    match (glyph, encoding) {
        (Some(g), _) => (base, 2, NamePtr(Some(g.clone())), Box::from([])),
        (None, Some(V::Array(a))) => (base, 1, NamePtr(None), a.borrow().iter().take(256).map(|v| NamePtr(encoded_name(v))).collect()),
        (None, Some(V::Proc(p))) => (base, 1, NamePtr(None), p.iter().take(256).map(|v| NamePtr(encoded_name(v))).collect()),
        _ => (base, 0, NamePtr(None), Box::from([])),
    }
}

/// The bytes a new drawable font keeps for `key`: the font, its table of 256 glyph names (each
/// its own copy; an unencoded code names `.notdef`), its base name and the key's list of names.
fn font_entry_bytes((base, glyphs): &FontKey) -> usize {
    let notdef = ".notdef".len();
    let (key_slots, names) = match glyphs {
        Glyphs::Encoding(v) => (v.len(), (0..256).map(|c| v.get(c).and_then(Option::as_ref).map_or(notdef, |n| n.len())).fold(0, usize::saturating_add)),
        Glyphs::One(n) => (0, n.len().saturating_add(255 * notdef)),
        Glyphs::Builtin => (0, 0),
    };
    std::mem::size_of::<Font>()
        .saturating_add(256 * std::mem::size_of::<Option<String>>())
        .saturating_add(base.len())
        .saturating_add(key_slots.saturating_mul(std::mem::size_of::<Option<Rc<str>>>()))
        .saturating_add(names)
}

fn dict_matrix(d: &DictRef, key: &str) -> Affine {
    d.borrow().get(key).and_then(matrix_of).unwrap_or(Affine::IDENTITY)
}

impl Interp {
    /// A copy of a font dictionary with `m` applied after its matrix (`scalefont`, `makefont`).
    /// The copy keeps an entry of its own for every key, and a new matrix, all charged first.
    fn transformed(&mut self, font: &DictRef, m: Affine) -> Result<DictRef, Flow> {
        for k in font.borrow().keys() {
            self.charge_entry(k)?;
        }
        let copy: HashMap<String, V> = font.borrow().clone();
        let d = Rc::new(RefCell::new(copy));
        let old = dict_matrix(&d, "__m");
        let matrix = self.matrix(m * old)?;
        self.dict_insert(&d, &V::Lit("__m".into()), matrix)?;
        Ok(d)
    }

    /// The font dictionary the key `k` names: one `definefont` made, else a new dictionary for an
    /// embedded program or a bundled stand-in, which Font Directory keeps.
    pub(super) fn find_font(&mut self, k: &V) -> Result<DictRef, Flow> {
        // Looking the name up in Font Directory hashes it; charge its scan (a string's or a name's)
        // so a loop of `findfont`/`selectfont` cannot do work that scales with its length each
        // time. A new font additionally keeps the name, length-capped and charged below.
        self.charge_scan(k)?;
        if let Some(V::Dict(d)) = k.with_key(|t| self.font_dir.borrow().get(t).cloned()) {
            return Ok(d);
        }
        // The new font keeps its name, once, as FontName and as the program to draw it with.
        let name: Rc<str> = self.converted_name(k)?;
        let d = new_dict();
        let entries = [
            ("FontName", V::Lit(name.clone())),
            ("FontType", V::Num(1.0)),
            ("FontMatrix", self.matrix(Affine::scale(0.001))?),
            ("FID", V::Null),
            ("__base", V::Lit(name)),
            ("__m", self.matrix(Affine::IDENTITY)?),
        ];
        for (key, v) in entries {
            self.dict_insert(&d, &V::Lit(key.into()), v)?;
        }
        self.define_font(k, V::Dict(d.clone()))?;
        Ok(d)
    }

    /// The drawable font of a font dictionary (its base program and the glyph names of its
    /// encoding, or the one `glyph` instead) and the matrix from text space (font size 1) to user
    /// space. A cheap pointer-identity cache answers the common case (the same dictionary drawn
    /// again) without hashing the glyph names; on a pointer miss the content-keyed cache dedups by
    /// what the names actually are, and a new font charges what it keeps ([`font_entry_bytes`] and
    /// its embedded program) before it is built.
    fn font_of(&mut self, d: &DictRef, glyph: Option<Rc<str>>) -> Result<Option<(Rc<Font>, Affine)>, Flow> {
        if matches!(d.borrow().get("FontType"), Some(V::Num(t)) if *t == 3.0) {
            // Type 3 fonts (glyph procedures) are not run.
            self.b.skip("EPS Type 3 font");
            return Ok(None);
        }
        let font = {
            let m = d.borrow();
            let Some(base) = m.get("__base").or_else(|| m.get("FontName")) else { return Ok(None) };
            // A string base is copied (and charged) each time, so a huge font name stays bounded.
            let base = self.name_of(base)?;
            let encoding = m.get("Encoding");
            // The pointers that identify this font's glyph names. Re-encoding makes fresh name
            // objects (charged), so their pointers change and the pointer cache misses.
            let fast = fast_key(&base, glyph.as_ref(), encoding);
            if let Some(f) = self.font_fast.get(&fast) {
                f.clone()
            } else {
                let glyphs = match (glyph, encoding) {
                    (Some(g), _) => Glyphs::One(g),
                    (None, Some(V::Array(a))) => Glyphs::Encoding(a.borrow().iter().take(256).map(encoded_name).collect()),
                    (None, Some(V::Proc(p))) => Glyphs::Encoding(p.iter().take(256).map(encoded_name).collect()),
                    _ => Glyphs::Builtin,
                };
                let key: FontKey = (base, glyphs);
                let f = match self.font_cache.get(&key) {
                    Some(f) => f.clone(),
                    None => {
                        self.allocate(font_entry_bytes(&key))?;
                        let (base, glyphs) = &key;
                        let names = match glyphs {
                            Glyphs::Encoding(v) => Some(v.iter().map(|x| x.as_deref().map(str::to_owned)).collect()),
                            Glyphs::One(g) => Some((0..=65).map(|c| (c == 65).then(|| g.to_string())).collect()),
                            Glyphs::Builtin => None,
                        };
                        // Not embedded: a bundled stand-in, as for PDF. The program bytes are shared,
                        // but building the font parses and keeps a result sized by them, so charge
                        // the program's length to bound a loop of new encodings over a big program.
                        let prog = self.embedded.get(&**base).cloned();
                        if let Some((data, _)) = &prog {
                            self.allocate(data.len())?;
                        }
                        let f = Rc::new(Font::standalone(base, prog.as_ref().map(|(d, cff)| (d.as_slice(), *cff)), names));
                        self.font_cache.insert(key, f.clone());
                        f
                    }
                };
                // The pointer cache only accelerates; the content cache above is the source of
                // truth, so clearing it when it grows large keeps its memory bounded.
                if self.font_fast.len() >= 1024 {
                    self.font_fast.clear();
                }
                self.font_fast.insert(fast, f.clone());
                f
            }
        };
        Ok(Some((font, dict_matrix(d, "__m"))))
    }

    /// A string's glyph outlines (device space) in the current font, or as the one `glyph` for
    /// code 65, from the current point, with an extra displacement `extra(index, code)` (user
    /// space) after each glyph; moves the current point.
    fn text_path(&mut self, glyph: Option<Rc<str>>, bytes: &[u8], mut extra: impl FnMut(usize, u32) -> (f64, f64)) -> Result<Option<(BezPath, String)>, Flow> {
        let Some(d) = self.gs.font.clone() else { return Ok(None) };
        let Some((font, m)) = self.font_of(&d, glyph)? else { return Ok(None) };
        let mut p = self.current_user();
        let mut path = BezPath::new();
        let mut text = String::new();
        let ctm = self.gs.ctm;
        for (i, &code) in bytes.iter().enumerate() {
            let code = code as u32;
            if let Some(g) = font.glyph(code) {
                // Each glyph's outline elements count against the operation budget and the path
                // limit, as the elements any operator builds do, so a long string's glyphs are
                // paid for and cannot grow the path without bound before it is used.
                let n = g.elements().len();
                self.spend(n)?;
                self.path_fits(path.elements().len().saturating_add(n))?;
                path.extend(ctm * Affine::translate(p.to_vec2()) * m * (*g).clone());
            }
            text.push_str(&font.unicode(code));
            let adv = m * Point::new(font.width(code), 0.0) - m * Point::ZERO;
            let (ex, ey) = extra(i, code);
            p += adv + kurbo::Vec2::new(ex, ey);
        }
        self.move_to(p)?;
        Ok(Some((path, text)))
    }

    fn fill_text(&mut self, path: BezPath, text: &str) {
        let label: String = text.chars().filter(|c| !c.is_control()).take(40).collect();
        let name = if label.trim().is_empty() { "Text".to_string() } else { format!("Text: {}", label.trim()) };
        self.b.fill_stroke_named(&name, path, Affine::IDENTITY, Some((Paint::Color(self.gs.color), 1.0, FillRule::NonZero)), None);
    }

    /// Pop and precharge a string-byte snapshot for text operators; nonstrings produce no bytes.
    fn pop_string(&mut self) -> Result<Vec<u8>, Flow> {
        Ok(match self.pop() {
            V::Str(s) => {
                self.allocate(s.borrow().len())?;
                s.borrow().clone()
            }
            _ => vec![],
        })
    }

    fn show_with(&mut self, glyph: Option<Rc<str>>, s: &[u8], extra: impl FnMut(usize, u32) -> (f64, f64)) -> Result<(), Flow> {
        match self.text_path(glyph, s, extra)? {
            Some((path, text)) => self.fill_text(path, &text),
            None => self.b.skip("text (no font)"),
        }
        Ok(())
    }

    /// Text and font operators (PostScript Language Reference §5.1, §8.2).
    pub(super) fn text_op(&mut self, n: &str, src: &mut Src) -> Result<(), Flow> {
        match n {
            "scalefont" | "makefont" => {
                let m = if n == "scalefont" {
                    Affine::scale(self.popn())
                } else {
                    let v = self.pop();
                    matrix_of(&v).unwrap_or(Affine::IDENTITY)
                };
                let font = match self.pop() {
                    V::Dict(d) => d,
                    _ => self.find_font(&V::Lit("Helvetica".into()))?,
                };
                let scaled = self.transformed(&font, m)?;
                self.push(V::Dict(scaled));
            }
            "selectfont" => {
                let size = self.pop();
                let key = self.pop();
                let font = self.find_font(&key)?;
                let m = match &size {
                    V::Num(s) => Affine::scale(*s),
                    other => matrix_of(other).unwrap_or(Affine::IDENTITY),
                };
                self.gs.font = Some(self.transformed(&font, m)?);
            }
            "setfont" => {
                if let V::Dict(d) = self.pop() {
                    self.gs.font = Some(d);
                }
            }
            "currentfont" | "rootfont" => {
                let d = self.gs.font.clone().unwrap_or_else(new_dict);
                self.push(V::Dict(d));
            }
            "definefont" => {
                let v = self.pop();
                let key = self.pop();
                if let V::Dict(d) = &v {
                    if !d.borrow().contains_key("__base") {
                        // A font the file defines itself: its embedded program, by name, which
                        // the font keeps as its own copy.
                        let name = self.converted_name(&key)?;
                        self.dict_insert(d, &V::Lit("__base".into()), V::Lit(name))?;
                        let matrix = self.matrix(Affine::IDENTITY)?;
                        self.dict_insert(d, &V::Lit("__m".into()), matrix)?;
                    }
                    self.define_font(&key, v.clone())?;
                }
                self.push(v);
            }
            "undefinefont" => {
                let key = self.pop();
                self.charge_scan(&key)?;
                key.with_key(|t| self.font_dir.borrow_mut().remove(t));
            }
            "StandardEncoding" | "ISOLatin1Encoding" => self.push_encoding(n)?,
            "show" => {
                let s = self.pop_string()?;
                self.show_with(None, &s, |_, _| (0.0, 0.0))?;
            }
            "ashow" => {
                let s = self.pop_string()?;
                let ay = self.popn();
                let ax = self.popn();
                self.show_with(None, &s, |_, _| (ax, ay))?;
            }
            "widthshow" => {
                let s = self.pop_string()?;
                let ch = self.popn() as u32;
                let cy = self.popn();
                let cx = self.popn();
                self.show_with(None, &s, |_, c| if c == ch { (cx, cy) } else { (0.0, 0.0) })?;
            }
            "awidthshow" => {
                let s = self.pop_string()?;
                let ay = self.popn();
                let ax = self.popn();
                let ch = self.popn() as u32;
                let cy = self.popn();
                let cx = self.popn();
                self.show_with(None, &s, |_, c| if c == ch { (cx + ax, cy + ay) } else { (ax, ay) })?;
            }
            "kshow" | "cshow" => {
                // The procedure between characters is not run.
                let s = self.pop_string()?;
                self.pop();
                self.show_with(None, &s, |_, _| (0.0, 0.0))?;
            }
            "xshow" | "yshow" | "xyshow" => {
                let v = self.pop();
                let disp: Vec<f64> = self.items(&v)?.iter().filter_map(V::num).collect();
                let s = self.pop_string()?;
                self.explicit_show(&s, &disp, n)?;
            }
            "glyphshow" => {
                let v = self.pop();
                if self.gs.font.is_some() {
                    // Drawn as code 65 of a font that names only that glyph, cached like any other.
                    let glyph = self.name_of(&v)?;
                    self.show_with(Some(glyph), b"A", |_, _| (0.0, 0.0))?;
                }
            }
            "charpath" => {
                self.pop();
                let s = self.pop_string()?;
                let saved = std::mem::take(&mut self.gs.path);
                let start = self.gs.start;
                match self.text_path(None, &s, |_, _| (0.0, 0.0))? {
                    Some((glyphs, _)) => {
                        // `text_path` already spent and path-limited each glyph's elements as it
                        // built them; joining them to the current path only needs room for them.
                        self.gs.path = saved;
                        self.room(glyphs.elements().len().saturating_add(1))?;
                        self.gs.path.extend(glyphs);
                        if let Some(c) = self.gs.cur {
                            self.gs.path.move_to(c);
                        }
                        self.gs.start = start;
                    }
                    None => self.gs.path = saved,
                }
            }
            "stringwidth" => {
                let s = self.pop_string()?;
                let (mut w, mut h) = (0.0, 0.0);
                if let Some(d) = self.gs.font.clone()
                    && let Some((font, m)) = self.font_of(&d, None)?
                {
                    for &c in &s {
                        let v = m * Point::new(font.width(c as u32), 0.0) - m * Point::ZERO;
                        w += v.x;
                        h += v.y;
                    }
                }
                self.push(V::Num(w));
                self.push(V::Num(h));
            }
            "setcachedevice" => {
                for _ in 0..6 {
                    self.pop();
                }
            }
            "setcharwidth" => {
                self.pop();
                self.pop();
            }
            // An embedded Type 1 program's encrypted part (read by `embedded_fonts`).
            "eexec" => {
                if matches!(self.pop(), V::File) {
                    let rest = &src.d[src.pos.min(src.d.len())..];
                    src.pos += rest.windows(11).position(|w| w == b"cleartomark").map_or(rest.len(), |p| p + 11);
                }
            }
            // A CFF font set's binary data (read by `embedded_fonts`).
            "StartData" => {
                let len = self.popn().max(0.0) as usize;
                self.pop();
                src.pos = src.pos.saturating_add(1).saturating_add(len).min(src.d.len());
            }
            _ => {}
        }
        Ok(())
    }

    /// `xshow` / `yshow` / `xyshow`: glyph displacements from a number array instead of the
    /// advances.
    fn explicit_show(&mut self, s: &[u8], disp: &[f64], n: &str) -> Result<(), Flow> {
        let step = if n == "xyshow" { 2 } else { 1 };
        // Glyph `k`'s displacement, read from `disp` as it is drawn.
        let at = move |k: usize| {
            let i = k.saturating_mul(step);
            let v = |j: usize| disp.get(j).copied().unwrap_or(0.0);
            match n {
                "xshow" => (v(i), 0.0),
                "yshow" => (0.0, v(i)),
                _ => (v(i), v(i.saturating_add(1))),
            }
        };
        let Some(fd) = self.gs.font.clone() else {
            self.b.skip("text (no font)");
            return Ok(());
        };
        let Some((font, m)) = self.font_of(&fd, None)? else { return Ok(()) };
        self.show_with(None, s, move |k, c| {
            let adv = m * Point::new(font.width(c), 0.0) - m * Point::ZERO;
            let (dx, dy) = at(k);
            (dx - adv.x, dy - adv.y)
        })
    }
}
