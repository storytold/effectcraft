use super::scanner::Src;
use super::value::{DictRef, Flow, V, array, matrix_of, matrix_val, new_dict};
use super::{Affine, BezPath, FillRule, Font, HashMap, Interp, Paint, Point, Rc, RefCell};

// ------------------------------------------------------------------ text

/// An encoding array of glyph names (`StandardEncoding`; `ISOLatin1Encoding` and others are
/// approximated by WinAnsi, which agrees with ISO Latin-1 on the printable codes).
pub(super) fn encoding_array(name: &str) -> V {
    let base = if name == "StandardEncoding" { crate::encoding::Base::Standard } else { crate::encoding::Base::WinAnsi };
    array((0..256).map(|c| V::Lit(base.name(c as u8).unwrap_or_else(|| ".notdef".into()).into())).collect())
}

fn dict_matrix(d: &DictRef, key: &str) -> Affine {
    d.borrow().get(key).and_then(matrix_of).unwrap_or(Affine::IDENTITY)
}

/// A copy of a font dictionary with `m` applied after its matrix (`scalefont`, `makefont`).
fn transformed(font: &DictRef, m: Affine) -> DictRef {
    let copy: HashMap<String, V> = font.borrow().clone();
    let d = Rc::new(RefCell::new(copy));
    let old = dict_matrix(&d, "__m");
    d.borrow_mut().insert("__m".into(), matrix_val(m * old));
    d
}

impl Interp {
    /// A font dictionary by name: one `definefont` made, else a new dictionary for an
    /// embedded program or a bundled stand-in.
    pub(super) fn find_font(&mut self, key: &str) -> DictRef {
        if let Some(d) = self.font_dir.get(key) {
            return d.clone();
        }
        let d = new_dict();
        {
            let mut m = d.borrow_mut();
            m.insert("FontName".into(), V::Lit(key.into()));
            m.insert("FontType".into(), V::Num(1.0));
            m.insert("FontMatrix".into(), matrix_val(Affine::scale(0.001)));
            m.insert("FID".into(), V::Null);
            m.insert("__base".into(), V::Lit(key.into()));
            m.insert("__m".into(), matrix_val(Affine::IDENTITY));
        }
        self.font_dir.insert(key.to_string(), d.clone());
        d
    }

    /// The drawable font of a font dictionary (its base program and encoding) and the matrix
    /// from text space (font size 1) to user space.
    fn font_of(&mut self, d: &DictRef) -> Option<(Rc<Font>, Affine)> {
        if matches!(d.borrow().get("FontType"), Some(V::Num(t)) if *t == 3.0) {
            // Type 3 fonts (glyph procedures) are not run.
            self.b.skip("EPS Type 3 font");
            return None;
        }
        let (base, names) = {
            let m = d.borrow();
            let base = m.get("__base").or_else(|| m.get("FontName")).map(V::key)?;
            let names: Option<Vec<Option<String>>> = m.get("Encoding").and_then(V::items).map(|a| {
                a.iter()
                    .map(|v| match v {
                        V::Lit(n) | V::Name(n) if &**n != ".notdef" => Some(n.to_string()),
                        _ => None,
                    })
                    .collect()
            });
            (base, names)
        };
        let key = match &names {
            Some(n) => format!("{base}\u{0}{}", n.iter().map(|x| x.as_deref().unwrap_or("")).collect::<Vec<_>>().join(" ")),
            None => base.clone(),
        };
        let font = match self.font_cache.get(&key) {
            Some(f) => f.clone(),
            None => {
                // Not embedded: a bundled stand-in, as for PDF.
                let prog = self.embedded.get(&base).cloned();
                let f = Rc::new(Font::standalone(&base, prog.as_ref().map(|(d, cff)| (d.as_slice(), *cff)), names));
                self.font_cache.insert(key, f.clone());
                f
            }
        };
        Some((font, dict_matrix(d, "__m")))
    }

    /// A string's glyph outlines (device space) from the current point, with an extra
    /// displacement `extra(index, code)` (user space) after each glyph; moves the current
    /// point.
    fn text_path(&mut self, bytes: &[u8], mut extra: impl FnMut(usize, u32) -> (f64, f64)) -> Result<Option<(BezPath, String)>, Flow> {
        let Some(d) = self.gs.font.clone() else { return Ok(None) };
        let Some((font, m)) = self.font_of(&d) else { return Ok(None) };
        let mut p = self.current_user();
        let mut path = BezPath::new();
        let mut text = String::new();
        let ctm = self.gs.ctm;
        for (i, &code) in bytes.iter().enumerate() {
            let code = code as u32;
            if let Some(g) = font.glyph(code) {
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

    fn pop_string(&mut self) -> Vec<u8> {
        match self.pop() {
            V::Str(s) => s.borrow().clone(),
            _ => vec![],
        }
    }

    fn show_with(&mut self, s: &[u8], extra: impl FnMut(usize, u32) -> (f64, f64)) -> Result<(), Flow> {
        match self.text_path(s, extra)? {
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
                    _ => self.find_font("Helvetica"),
                };
                self.push(V::Dict(transformed(&font, m)));
            }
            "selectfont" => {
                let size = self.pop();
                let key = self.pop().key();
                let font = self.find_font(&key);
                let m = match &size {
                    V::Num(s) => Affine::scale(*s),
                    other => matrix_of(other).unwrap_or(Affine::IDENTITY),
                };
                self.gs.font = Some(transformed(&font, m));
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
                let key = self.pop().key();
                if let V::Dict(d) = &v {
                    if !d.borrow().contains_key("__base") {
                        // A font the file defines itself: its embedded program, by name.
                        d.borrow_mut().insert("__base".into(), V::Lit(key.as_str().into()));
                        d.borrow_mut().insert("__m".into(), matrix_val(Affine::IDENTITY));
                    }
                    self.font_dir.insert(key, d.clone());
                }
                self.push(v);
            }
            "undefinefont" => {
                let key = self.pop().key();
                self.font_dir.remove(&key);
            }
            "StandardEncoding" | "ISOLatin1Encoding" => self.push(encoding_array(n)),
            "show" => {
                let s = self.pop_string();
                self.show_with(&s, |_, _| (0.0, 0.0))?;
            }
            "ashow" => {
                let s = self.pop_string();
                let ay = self.popn();
                let ax = self.popn();
                self.show_with(&s, |_, _| (ax, ay))?;
            }
            "widthshow" => {
                let s = self.pop_string();
                let ch = self.popn() as u32;
                let cy = self.popn();
                let cx = self.popn();
                self.show_with(&s, |_, c| if c == ch { (cx, cy) } else { (0.0, 0.0) })?;
            }
            "awidthshow" => {
                let s = self.pop_string();
                let ay = self.popn();
                let ax = self.popn();
                let ch = self.popn() as u32;
                let cy = self.popn();
                let cx = self.popn();
                self.show_with(&s, |_, c| if c == ch { (cx + ax, cy + ay) } else { (ax, ay) })?;
            }
            "kshow" | "cshow" => {
                // The procedure between characters is not run.
                let s = self.pop_string();
                self.pop();
                self.show_with(&s, |_, _| (0.0, 0.0))?;
            }
            "xshow" | "yshow" | "xyshow" => {
                let disp: Vec<f64> = self.pop().items().unwrap_or_default().iter().filter_map(V::num).collect();
                let s = self.pop_string();
                self.explicit_show(&s, &disp, n)?;
            }
            "glyphshow" => {
                let name = self.pop().key();
                let Some(d) = self.gs.font.clone() else { return Ok(()) };
                // Drawn through a one-glyph encoding.
                let mut e: Vec<V> = (0..256).map(|_| V::Lit(".notdef".into())).collect();
                e[65] = V::Lit(name.as_str().into());
                let tmp = transformed(&d, Affine::IDENTITY);
                tmp.borrow_mut().insert("Encoding".into(), array(e));
                self.gs.font = Some(tmp);
                self.show_with(b"A", |_, _| (0.0, 0.0))?;
                self.gs.font = Some(d);
            }
            "charpath" => {
                self.pop();
                let s = self.pop_string();
                let saved = std::mem::take(&mut self.gs.path);
                let start = self.gs.start;
                match self.text_path(&s, |_, _| (0.0, 0.0))? {
                    Some((glyphs, _)) => {
                        // The outlines join the path: each element counts against the budget and
                        // the path limit.
                        self.gs.path = saved;
                        self.room(glyphs.elements().len() + 1)?;
                        self.spend(glyphs.elements().len())?;
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
                let s = self.pop_string();
                let (mut w, mut h) = (0.0, 0.0);
                if let Some(d) = self.gs.font.clone()
                    && let Some((font, m)) = self.font_of(&d)
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
        let d: Vec<(f64, f64)> = (0..s.len())
            .map(|k| {
                let i = k * step;
                let v = |j: usize| disp.get(j).copied().unwrap_or(0.0);
                match n {
                    "xshow" => (v(i), 0.0),
                    "yshow" => (0.0, v(i)),
                    _ => (v(i), v(i + 1)),
                }
            })
            .collect();
        let Some(fd) = self.gs.font.clone() else {
            self.b.skip("text (no font)");
            return Ok(());
        };
        let Some((font, m)) = self.font_of(&fd) else { return Ok(()) };
        self.show_with(s, move |k, c| {
            let adv = m * Point::new(font.width(c), 0.0) - m * Point::ZERO;
            let (dx, dy) = d.get(k).copied().unwrap_or((0.0, 0.0));
            (dx - adv.x, dy - adv.y)
        })
    }
}
