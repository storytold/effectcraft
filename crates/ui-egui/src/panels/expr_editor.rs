//! The expression editor of the Timeline, following Settings ▸ Scripting & Expressions:
//! Font Size, Syntax Highlighting, Line Numbers, Auto-complete (Tab accepts), Bracket Matching
//! and Word Wrap. The Composition viewer uses the shared diagnostics in `expr_bar`.

use effectcraft_engine::commands::expr_tools::language_menu;
use effectcraft_engine::prefs::Scripting;
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, Rect, Stroke, pos2, vec2};

use crate::theme::Tokens;

/// Syntax colours, and the matched brackets' background.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Syntax {
    pub keyword: Color32,
    pub number: Color32,
    pub string: Color32,
    pub comment: Color32,
    pub api: Color32,
    pub bracket_bg: Color32,
}

impl Syntax {
    /// The colours for the theme: on the Light theme's light expression field, darker ones that
    /// stay readable (#644).
    pub fn for_tokens(t: &Tokens) -> Syntax {
        if t.kind.is_light() {
            Syntax {
                keyword: Color32::from_rgb(0x7a, 0x2f, 0xa8),
                number: Color32::from_rgb(0x8a, 0x4b, 0x00),
                string: Color32::from_rgb(0x2c, 0x6a, 0x10),
                comment: Color32::from_rgb(0x52, 0x5a, 0x62),
                api: Color32::from_rgb(0x0a, 0x57, 0x94),
                bracket_bg: Color32::from_rgb(0xd0, 0xdc, 0xf0),
            }
        } else {
            Syntax {
                keyword: Color32::from_rgb(0xc6, 0x8a, 0xe6),
                number: Color32::from_rgb(0xe0, 0xb4, 0x6c),
                string: Color32::from_rgb(0x9c, 0xd0, 0x7a),
                comment: Color32::from_rgb(0x80, 0x88, 0x90),
                api: Color32::from_rgb(0x6c, 0xb8, 0xf0),
                bracket_bg: Color32::from_rgb(0x4a, 0x55, 0x6a),
            }
        }
    }
}

const KEYWORDS: &[&str] = &[
    "var",
    "let",
    "const",
    "function",
    "return",
    "if",
    "else",
    "for",
    "while",
    "do",
    "break",
    "continue",
    "new",
    "true",
    "false",
    "null",
    "undefined",
    "this",
    "typeof",
    "in",
    "of",
    "switch",
    "case",
    "default",
    "try",
    "catch",
    "throw",
];

/// Expression language names offered by auto-complete (public Expression Language Reference
/// vocabulary).
pub const COMPLETIONS: &[&str] = &[
    "thisComp",
    "thisLayer",
    "thisProperty",
    "time",
    "value",
    "index",
    "layer",
    "effect",
    "mask",
    "content",
    "transform",
    "position",
    "anchorPoint",
    "scale",
    "rotation",
    "opacity",
    "orientation",
    "xRotation",
    "yRotation",
    "zRotation",
    "wiggle",
    "loopOut",
    "loopIn",
    "loopOutDuration",
    "loopInDuration",
    "valueAtTime",
    "velocityAtTime",
    "speedAtTime",
    "linear",
    "ease",
    "easeIn",
    "easeOut",
    "clamp",
    "random",
    "gaussRandom",
    "seedRandom",
    "noise",
    "degreesToRadians",
    "radiansToDegrees",
    "timeToFrames",
    "framesToTime",
    "timeToTimecode",
    "length",
    "lookAt",
    "toComp",
    "fromComp",
    "toWorld",
    "fromWorld",
    "sourceRectAtTime",
    "sourceText",
    "numKeys",
    "key",
    "nearestKey",
    "inPoint",
    "outPoint",
    "startTime",
    "width",
    "height",
    "frameDuration",
    "marker",
    "Math",
    "comp",
    "footage",
    "posterizeTime",
    "add",
    "sub",
    "mul",
    "div",
    "normalize",
    "cross",
    "dot",
    "hslToRgb",
    "rgbToHsl",
    "createPath",
    "points",
    "inTangents",
    "outTangents",
    "isClosed",
];

/// The bracket matching the one at or just before char index `cursor`: (open, close) char
/// indices.
pub fn matching_bracket(text: &str, cursor: usize) -> Option<(usize, usize)> {
    let chars: Vec<char> = text.chars().collect();
    let pairs = [('(', ')'), ('[', ']'), ('{', '}')];
    for at in [cursor, cursor.wrapping_sub(1)] {
        let Some(&c) = chars.get(at) else { continue };
        if let Some(&(o, cl)) = pairs.iter().find(|(o, _)| *o == c) {
            let mut depth = 0;
            for (i, &x) in chars.iter().enumerate().skip(at) {
                if x == o {
                    depth += 1;
                } else if x == cl {
                    depth -= 1;
                    if depth == 0 {
                        return Some((at, i));
                    }
                }
            }
        }
        if let Some(&(o, cl)) = pairs.iter().find(|(_, cl)| *cl == c) {
            let mut depth = 0;
            for i in (0..=at).rev() {
                if chars[i] == cl {
                    depth += 1;
                } else if chars[i] == o {
                    depth -= 1;
                    if depth == 0 {
                        return Some((i, at));
                    }
                }
            }
        }
    }
    None
}

/// The identifier being typed before char index `cursor`, and auto-complete candidates for it.
pub fn completions(text: &str, cursor: usize) -> (String, Vec<&'static str>) {
    let chars: Vec<char> = text.chars().collect();
    let end = cursor.min(chars.len());
    let mut start = end;
    while start > 0 && (chars[start - 1].is_alphanumeric() || chars[start - 1] == '_') {
        start -= 1;
    }
    let word: String = chars[start..end].iter().collect();
    if word.chars().count() < 2 || word.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return (word, vec![]);
    }
    let c: Vec<&str> = COMPLETIONS.iter().copied().filter(|w| w.starts_with(word.as_str()) && *w != word).take(6).collect();
    (word, c)
}

/// How `name` is called, from the Expression Language menu's signatures: `None` for a
/// property, `Some(true)` for a function that takes arguments, `Some(false)` for one that never
/// does.
fn call_shape(name: &str) -> Option<bool> {
    let calls: Vec<&str> =
        language_menu().into_iter().flat_map(|(_, items)| items).filter_map(|(_, text)| text.strip_prefix(name)?.strip_prefix('(')).collect();
    (!calls.is_empty()).then(|| calls.iter().any(|rest| !rest.starts_with(')')))
}

/// Accepts the first auto-complete candidate at char index `cursor`: inserts the rest of the
/// name, plus `()` for a function (#1393), and returns the new cursor: between the parentheses,
/// or after them when the function takes no arguments.
pub fn accept_completion(buf: &mut String, cursor: usize) -> Option<usize> {
    let (word, cands) = completions(buf, cursor);
    let first = cands.first()?;
    let at = byte_of(buf, cursor);
    let rest = &first[word.len()..];
    let end = cursor + first.chars().count() - word.chars().count();
    let already_called = buf.get(at..).is_some_and(|after| after.starts_with('('));
    match call_shape(first).filter(|_| !already_called) {
        Some(takes_args) => {
            buf.insert_str(at, &format!("{rest}()"));
            Some(end + if takes_args { 1 } else { 2 })
        }
        None => {
            buf.insert_str(at, rest);
            Some(end)
        }
    }
}

/// Colour runs for JavaScript: (byte start, byte end, colour).
pub fn highlight(text: &str, base: Color32, sx: &Syntax) -> Vec<(usize, usize, Color32)> {
    let b = text.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let start = i;
        let col = if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            sx.comment
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            while i < b.len() && !(b[i] == b'*' && b.get(i + 1) == Some(&b'/')) {
                i += 1;
            }
            i = (i + 2).min(b.len());
            sx.comment
        } else if c == b'"' || c == b'\'' || c == b'`' {
            i += 1;
            while i < b.len() && b[i] != c {
                i += if b[i] == b'\\' { 2 } else { 1 };
            }
            i = (i + 1).min(b.len());
            sx.string
        } else if c.is_ascii_digit() {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'.') {
                i += 1;
            }
            sx.number
        } else if c.is_ascii_alphabetic() || c == b'_' || c == b'$' {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'$') {
                i += 1;
            }
            let w = &text[start..i];
            if KEYWORDS.contains(&w) {
                sx.keyword
            } else if COMPLETIONS.contains(&w) {
                sx.api
            } else {
                base
            }
        } else {
            // Advance one whole character.
            i += text[i..].chars().next().map_or(1, char::len_utf8);
            base
        };
        match out.last_mut() {
            Some((_, e, c)) if *c == col && *e == start => *e = i,
            _ => out.push((start, i, col)),
        }
    }
    out
}

/// Byte index of char index `ci`.
fn byte_of(text: &str, ci: usize) -> usize {
    text.char_indices().nth(ci).map(|(b, _)| b).unwrap_or(text.len())
}

/// The layout job of the editor text: syntax colours, the matched brackets' background, wrap.
fn layout(text: &str, font: FontId, base: Color32, sx: &Syntax, p: &Scripting, brackets: Option<(usize, usize)>, wrap: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    let runs = if p.syntax_highlighting { highlight(text, base, sx) } else { vec![(0, text.len(), base)] };
    let marks: Vec<usize> = brackets.filter(|_| p.bracket_matching).map(|(a, b)| vec![byte_of(text, a), byte_of(text, b)]).unwrap_or_default();
    for (s, e, col) in runs {
        // Split runs at the marked brackets.
        let mut cuts = vec![s];
        for m in &marks {
            if *m >= s && *m < e {
                cuts.push(*m);
                cuts.push((*m + 1).min(e));
            }
        }
        cuts.push(e);
        cuts.sort_unstable();
        cuts.dedup();
        for w in cuts.windows(2) {
            let bg = if marks.contains(&w[0]) { sx.bracket_bg } else { Color32::TRANSPARENT };
            job.append(&text[w[0]..w[1]], 0.0, TextFormat { font_id: font.clone(), color: col, background: bg, ..Default::default() });
        }
    }
    job.wrap.max_width = if p.word_wrap { wrap } else { f32::INFINITY };
    job
}

/// The editor: returns the response; `buf` is edited in place.
pub fn editor(ui: &mut egui::Ui, id: egui::Id, buf: &mut String, rect: Rect, p: &Scripting, color: Color32, rows: usize, t: &Tokens) -> egui::Response {
    let size = (p.editor_font_size.clamp(8, 40) as f32) * 0.9;
    let font = Tokens::mono(size);
    // Line numbers in a gutter on the left.
    let gutter = if p.line_numbers { (size * 0.62) * (buf.lines().count().max(1).to_string().len() as f32 + 1.0) + 6.0 } else { 0.0 };
    let er = Rect::from_min_max(pos2(rect.min.x + gutter, rect.min.y), rect.max);
    let cursor = egui::TextEdit::load_state(ui.ctx(), id).and_then(|s| s.cursor.char_range()).map(|r| r.primary.index.0);
    let brackets = cursor.and_then(|c| matching_bracket(buf, c));
    let pc = p.clone();
    let sx = Syntax::for_tokens(t);
    let mut layouter = move |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap: f32| {
        let job = layout(text.as_str(), font.clone(), color, &sx, &pc, brackets, wrap);
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(er));
    // Auto-complete: Tab accepts the first candidate (consumed before the editor sees it).
    let mut accepted = None;
    if p.auto_complete
        && child.memory(|m| m.has_focus(id))
        && let Some(c) = cursor
        && !completions(buf, c).1.is_empty()
        && child.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab))
    {
        accepted = accept_completion(buf, c);
    }
    let out = egui::TextEdit::multiline(buf)
        .id(id)
        .font(Tokens::mono(size))
        .desired_width(er.width())
        .desired_rows(rows)
        .frame(egui::Frame::NONE)
        .lock_focus(true)
        .layouter(&mut layouter)
        .show(&mut child);
    if let Some(c) = accepted {
        let mut st = out.state.clone();
        st.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(c))));
        st.store(ui.ctx(), id);
    }
    let painter = ui.painter();
    if p.line_numbers {
        let g = out.galley.clone();
        let mut line = 1;
        let mut new_line = true;
        for row in &g.rows {
            if new_line {
                let y = out.galley_pos.y + row.rect().center().y;
                painter.text(pos2(rect.min.x + gutter - 6.0, y), egui::Align2::RIGHT_CENTER, line.to_string(), Tokens::mono(size * 0.85), t.text_faint);
                line += 1;
            }
            new_line = row.ends_with_newline;
        }
        painter.line_segment([pos2(rect.min.x + gutter - 3.0, rect.min.y), pos2(rect.min.x + gutter - 3.0, rect.max.y)], Stroke::new(1.0, t.separator));
    }
    // Candidate list under the editor.
    if p.auto_complete
        && out.response.has_focus()
        && let Some(c) = out.cursor_range.map(|r| r.primary.index.0)
    {
        let (_, cands) = completions(buf, c);
        if !cands.is_empty() {
            let pos = out.galley_pos + out.galley.pos_from_cursor(egui::text::CCursor::new(c)).left_bottom().to_vec2() + vec2(0.0, 2.0);
            egui::Area::new(id.with("complete")).order(egui::Order::Tooltip).fixed_pos(pos).show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    for (i, w) in cands.iter().enumerate() {
                        let txt = if i == 0 { format!("{w}    Tab") } else { w.to_string() };
                        ui.label(egui::RichText::new(txt).font(Tokens::mono(size * 0.9)).color(if i == 0 { t.text } else { t.text_dim }));
                    }
                });
            });
        }
    }
    out.response.response
}

#[cfg(test)]
mod contrast_tests {
    use super::*;
    use crate::theme::ThemeKind;

    /// WCAG contrast ratio of two opaque sRGB colours.
    fn contrast(a: Color32, b: Color32) -> f32 {
        let lum = |c: Color32| {
            let f = |v: u8| {
                let v = v as f32 / 255.0;
                if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
        };
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// #644: on every theme, effect names stay readable on Effect Controls' headers, and the
    /// expression field's text (every syntax colour, enabled and disabled expressions, matched
    /// brackets, line numbers) on its background; the switch icons show on their wells.
    #[test]
    fn custom_painted_surfaces_keep_text_readable_on_every_theme() {
        for kind in [ThemeKind::Dark, ThemeKind::Darker, ThemeKind::Light] {
            let t = Tokens::for_kind(kind);
            let sx = Syntax::for_tokens(&t);
            for (what, bg) in [("effect header", t.fx_header), ("active effect header", t.fx_header_active)] {
                assert!(contrast(t.text, bg) >= 4.5, "{kind:?}: effect name on {what}: {:.2}", contrast(t.text, bg));
            }
            assert!(contrast(t.icon, t.switch_well) >= 3.0, "{kind:?}: switch icons on their wells");
            if !kind.is_light() {
                continue;
            }
            let fg = [
                ("keyword", sx.keyword),
                ("number", sx.number),
                ("string", sx.string),
                ("comment", sx.comment),
                ("api", sx.api),
                ("enabled expression", t.expr_text),
                ("disabled expression", t.text_dim),
                ("line numbers", t.text_faint),
            ];
            for (what, c) in fg {
                assert!(contrast(c, t.expr_bg) >= 4.5, "{kind:?}: {what} on the expression field: {:.2}", contrast(c, t.expr_bg));
                assert!(contrast(c, sx.bracket_bg) >= 4.0, "{kind:?}: {what} on a matched bracket: {:.2}", contrast(c, sx.bracket_bg));
            }
            // The dark theme's syntax colours would not do on the light field.
            let dark = Syntax::for_tokens(&Tokens::for_kind(ThemeKind::Dark));
            assert!([dark.keyword, dark.number, dark.string, dark.api].iter().all(|c| contrast(*c, t.expr_bg) < 4.5));
        }
    }

    /// Syntax highlighting uses the theme's colours.
    #[test]
    fn highlight_uses_the_theme_palette() {
        let t = Tokens::for_kind(ThemeKind::Light);
        let sx = Syntax::for_tokens(&t);
        let text = "var amount = 12; // note";
        let runs = highlight(text, t.expr_text, &sx);
        let col = |s: &str| runs.iter().find(|(a, e, _)| text.get(*a..*e) == Some(s)).map(|r| r.2);
        assert_eq!(col("var"), Some(sx.keyword));
        assert_eq!(col("12"), Some(sx.number));
        assert_eq!(col("// note"), Some(sx.comment));
    }
}

#[cfg(test)]
mod completion_tests {
    use super::*;

    /// #1393: accepting a function inserts the call, with the cursor between the parentheses
    /// (after them when the function takes no arguments); a property stays a bare name.
    #[test]
    fn accepting_a_function_completion_inserts_the_call() {
        let mut buf = String::from("x = loopO");
        assert_eq!(accept_completion(&mut buf, 9), Some(12));
        assert_eq!(buf, "x = loopOut()");

        let mut buf = String::from("wigg");
        assert_eq!(accept_completion(&mut buf, 4), Some(7));
        assert_eq!(buf, "wiggle()");

        let mut buf = String::from("isClo");
        assert_eq!(accept_completion(&mut buf, 5), Some(10));
        assert_eq!(buf, "isClosed()");

        let mut buf = String::from("tim");
        assert_eq!(accept_completion(&mut buf, 3), Some(4));
        assert_eq!(buf, "time");
    }

    /// Completing a name already followed by its call does not add a second pair.
    #[test]
    fn accepting_before_an_existing_call_keeps_one_pair_of_parentheses() {
        let mut buf = String::from("loopO(\"cycle\")");
        assert_eq!(accept_completion(&mut buf, 5), Some(7));
        assert_eq!(buf, "loopOut(\"cycle\")");
    }
}
