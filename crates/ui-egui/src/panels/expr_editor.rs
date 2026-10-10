//! The expression editor of the Timeline, following Settings ▸ Scripting & Expressions:
//! Font Size, Syntax Highlighting, Line Numbers, Auto-complete (Tab accepts), Bracket Matching
//! and Word Wrap; and the Composition panel's expression error banner (Show Expression Error
//! Banner).

use effectcraft_engine::prefs::Scripting;
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, Rect, Stroke, pos2, vec2};

use crate::theme::Tokens;

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

/// Colour runs for JavaScript in the theme's syntax colours: (byte start, byte end, colour).
pub fn highlight(text: &str, base: Color32, t: &Tokens) -> Vec<(usize, usize, Color32)> {
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
            t.expr_comment
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            while i < b.len() && !(b[i] == b'*' && b.get(i + 1) == Some(&b'/')) {
                i += 1;
            }
            i = (i + 2).min(b.len());
            t.expr_comment
        } else if c == b'"' || c == b'\'' || c == b'`' {
            i += 1;
            while i < b.len() && b[i] != c {
                i += if b[i] == b'\\' { 2 } else { 1 };
            }
            i = (i + 1).min(b.len());
            t.expr_string
        } else if c.is_ascii_digit() {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'.') {
                i += 1;
            }
            t.expr_number
        } else if c.is_ascii_alphabetic() || c == b'_' || c == b'$' {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'$') {
                i += 1;
            }
            let w = &text[start..i];
            if KEYWORDS.contains(&w) {
                t.expr_keyword
            } else if COMPLETIONS.contains(&w) {
                t.expr_api
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
fn layout(text: &str, font: FontId, base: Color32, t: &Tokens, p: &Scripting, brackets: Option<(usize, usize)>, wrap: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    let runs = if p.syntax_highlighting { highlight(text, base, t) } else { vec![(0, text.len(), base)] };
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
            let bg = if marks.contains(&w[0]) { t.expr_bracket_bg } else { Color32::TRANSPARENT };
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
    let (pc, tc) = (p.clone(), *t);
    let mut layouter = move |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap: f32| {
        let job = layout(text.as_str(), font.clone(), color, &tc, &pc, brackets, wrap);
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(er));
    // Auto-complete: Tab accepts the first candidate (consumed before the editor sees it).
    let mut accepted = None;
    if p.auto_complete
        && child.memory(|m| m.has_focus(id))
        && let Some(c) = cursor
    {
        let (word, cands) = completions(buf, c);
        if let Some(first) = cands.first()
            && child.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab))
        {
            let at = byte_of(buf, c);
            buf.insert_str(at, &first[word.len()..]);
            accepted = Some(c + first.chars().count() - word.chars().count());
        }
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

/// Settings ▸ Scripting & Expressions ▸ Show Expression Error Banner: the first failing
/// expression of the comp at the current time, in a banner along the bottom of the viewer
/// (click it to reveal the property).
pub fn error_banner(app: &mut crate::EffectcraftApp, ui: &egui::Ui, area: Rect) {
    if !app.session.prefs.scripting.error_banner {
        return;
    }
    let Some(cid) = app.session.active_comp_id() else { return };
    let key = (app.session.revision, cid.0, app.session.time().0);
    let id = egui::Id::new("expr-error-banner");
    let cached: Option<((u64, u64, i64), Vec<serde_json::Value>)> = ui.ctx().data(|d| d.get_temp(id));
    let errors = match cached {
        Some((k, e)) if k == key => e,
        _ => {
            let e = app.session.expression_errors(cid);
            ui.ctx().data_mut(|d| d.insert_temp(id, (key, e.clone())));
            e
        }
    };
    let Some(first) = errors.first() else { return };
    let layer = first["layer"].as_u64().unwrap_or(0);
    let lname = app.session.active_comp().and_then(|c| c.layer(effectcraft_engine::project::LayerId(layer))).map(|l| l.name.clone()).unwrap_or_default();
    let msg = format!(
        "⚠ Expression error{}: {} ▸ {}: {}",
        if errors.len() > 1 { format!(" (1 of {})", errors.len()) } else { String::new() },
        lname,
        first["name"].as_str().unwrap_or(""),
        first["error"].as_str().unwrap_or("").lines().next().unwrap_or("")
    );
    let r = Rect::from_min_max(pos2(area.min.x, area.max.y - 22.0), area.max);
    let p = ui.painter();
    p.rect_filled(r, 0.0, Color32::from_rgb(0xf0, 0xc0, 0x30));
    p.text(pos2(r.min.x + 8.0, r.center().y), egui::Align2::LEFT_CENTER, msg, Tokens::ui(11.5), Color32::BLACK);
    app.auto.add("viewer.expressionErrorBanner", r, "Expression error banner");
    if ui.interact(r, id.with("click"), egui::Sense::click()).clicked() {
        let p = serde_json::json!({"props": [{"layer": layer, "prop": first["prop"]}]});
        if let Err(e) = crate::menus::invoke(app, ui.ctx(), "timeline.revealProps", p) {
            app.ui.status = e;
        }
    }
}
