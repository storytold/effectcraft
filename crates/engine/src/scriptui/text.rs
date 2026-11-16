//! ScriptUI text: one layout serves both `measureString` and `drawString` (#3782).
//!
//! [`UiText::layout`] lays text out with the text engine (shaping, bidi, line breaking and the
//! per-character font fallback that composition text uses). `measureString` reports its
//! [`UiText::size`]; frontends paint its [`UiText::coverage`] instead of laying the string out a
//! second time with their own fonts, so the measured box and the painted glyphs come from the same
//! faces, advances and rows, in the CLI, the desktop app and on the web.

use effectcraft_geom::Mat3;
use effectcraft_text::layout::{Layout, Line, ParagraphStyle, TextStyle};
use effectcraft_text::{fonts, glyph_outline};
use serde::{Deserialize, Serialize};

/// The face ScriptUI text uses before fallback: a bundled face, so the primary face is the same on
/// every platform.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UiFont {
    #[default]
    Regular,
    Bold,
    Italic,
    /// Monospaced families (`Courier New`, `Consolas`, `Menlo`…). JetBrains Mono has one style.
    Mono,
}

impl UiFont {
    /// The face for a ScriptUI font's family name and style (`REGULAR`, `BOLD`, `ITALIC`,
    /// `BOLDITALIC`, or a `ScriptUI.FontStyle` number). Bold italic draws bold: no bold italic
    /// face is bundled.
    pub fn resolve(family: &str, style: &str) -> Self {
        let family = family.to_ascii_lowercase();
        if ["mono", "courier", "consolas", "menlo", "monaco"].iter().any(|m| family.contains(m)) {
            return Self::Mono;
        }
        let style = style.trim().to_ascii_lowercase();
        if style.contains("bold") || style == "1" || style == "3" {
            Self::Bold
        } else if style.contains("italic") || style == "2" {
            Self::Italic
        } else {
            Self::Regular
        }
    }

    /// The family and style the text engine knows the face by.
    fn family_style(self) -> (&'static str, &'static str) {
        match self {
            Self::Regular => ("Inter", "Regular"),
            Self::Bold => ("Inter", "Bold"),
            Self::Italic => ("Inter", "Italic"),
            Self::Mono => ("JetBrains Mono", "Regular"),
        }
    }
}

/// The longest ScriptUI string `measureString` and `drawString` accept, in UTF-16 code units (a
/// JavaScript string's `length`). Longer strings are an error for both, never cut short.
pub const MAX_UI_TEXT_UNITS: usize = 64 * 1024;
/// The largest ScriptUI font size in pixels.
pub const MAX_UI_FONT_SIZE: f64 = 1000.0;
/// The largest coverage window [`UiText::coverage`] fills, in pixels per side: 4 MiB of coverage.
/// Frontends paint larger areas in tiles of at most this size.
pub const MAX_COVERAGE_SIDE: u32 = 1024;

/// Why ScriptUI text was not laid out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiTextError {
    /// The string has more than [`MAX_UI_TEXT_UNITS`] UTF-16 code units.
    TooLong { units: usize },
}

impl std::fmt::Display for UiTextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLong { units } => write!(f, "ScriptUI text is {units} characters long; at most {MAX_UI_TEXT_UNITS} can be measured or drawn"),
        }
    }
}

impl std::error::Error for UiTextError {}

/// The pixel size ScriptUI text is laid out at: `size` up to [`MAX_UI_FONT_SIZE`], 12 when it is
/// not a positive number.
fn ui_font_size(size: f64) -> f64 {
    if size.is_finite() && size > 0.0 { size.min(MAX_UI_FONT_SIZE) } else { 12.0 }
}

/// One row of laid-out ScriptUI text.
#[derive(Clone, Debug, PartialEq)]
pub struct UiLine {
    /// Byte range of the row in the text (without its line break).
    pub range: std::ops::Range<usize>,
    /// Top of the row and its baseline, from the top of the text.
    pub top: f64,
    pub baseline: f64,
    pub height: f64,
    /// The row's advance: a row that ends its paragraph keeps its trailing spaces, a wrapped row
    /// ends at its last visible character.
    pub width: f64,
}

/// ScriptUI text laid out once for measuring and painting.
#[derive(Clone, Debug)]
pub struct UiText {
    layout: Layout,
    lines: Vec<UiLine>,
    /// Per layout line: how far its glyphs move to start the row at x = 0 and to put them on the
    /// row's baseline.
    shifts: Vec<(f64, f64)>,
    size: [f64; 2],
    font_size: f64,
}

impl UiText {
    /// Lay out `text` in `font` at `size` pixels, wrapped at `wrap` pixels when
    /// that is a positive number. Lines break at line breaks, and when wrapping at the line break
    /// opportunities of Unicode's line breaking algorithm; a word wider than `wrap` is not split.
    /// Every row starts at x = 0, whatever the paragraph's direction. A row is as tall as the
    /// primary face's ascent, descent and line gap, and grows when a fallback face reaches higher or
    /// lower, rounded to whole pixels.
    pub fn layout(text: &str, font: UiFont, size: f64, wrap: Option<f64>) -> Result<Self, UiTextError> {
        // Byte length first: a string with fewer bytes than the limit has fewer code units too.
        if text.len() > MAX_UI_TEXT_UNITS {
            let units = text.encode_utf16().count();
            if units > MAX_UI_TEXT_UNITS {
                return Err(UiTextError::TooLong { units });
            }
        }
        let font_size = ui_font_size(size);
        let (family, style) = font.family_style();
        let style = TextStyle { family: family.into(), style: style.into(), size: font_size as f32, ..Default::default() };
        let para = ParagraphStyle { width: wrap.filter(|w| w.is_finite() && *w > 0.0).map(|w| w.min(1e9) as f32), ..Default::default() };
        // Uncached: scripts lay out arbitrary strings, which must not crowd text layers' layouts
        // out of the shared cache.
        let layout = effectcraft_text::layout::layout_uncached(text, &style, &para);
        let primary = fonts::face(fonts::resolve(family, style.style.as_str()).face).metrics(font_size as f32);
        let (asc, desc, gap) = (f64::from(primary.ascent), f64::from(primary.descent), f64::from(primary.line_gap.max(0.0)));
        let mut lines = Vec::with_capacity(layout.lines.len());
        let mut shifts = Vec::with_capacity(layout.lines.len());
        let mut top = 0.0;
        for l in &layout.lines {
            let (lo, hi) = caret_span(l);
            let width = if l.para_end { f64::from(l.width.max(hi - lo)) } else { f64::from(l.width) };
            let ascent = asc.max(f64::from(l.ascent));
            // Whole pixels, so n rows measure n times one row.
            let height = (ascent + desc.max(f64::from(l.descent)) + gap).round().max(1.0);
            let baseline = top + ascent;
            let left = if l.carets.is_empty() { f64::from(l.x) } else { f64::from(lo) };
            shifts.push((-left, baseline - f64::from(l.baseline)));
            lines.push(UiLine { range: l.range.clone(), top, baseline, height, width });
            top += height;
        }
        if lines.is_empty() {
            // `layout_uncached` returns no lines only for input past its own limit, which the
            // check above already rejects; keep one empty row regardless.
            top = (asc + desc + gap).round().max(1.0);
            lines.push(UiLine { range: 0..0, top: 0.0, baseline: asc, height: top, width: 0.0 });
        }
        let width = lines.iter().map(|l| l.width).fold(0.0, f64::max);
        Ok(Self { layout, lines, shifts, size: [width, top], font_size })
    }

    /// `[width, height]`: the widest row and the rows' total height, in pixels.
    pub fn size(&self) -> [f64; 2] {
        self.size
    }

    /// The rows, top to bottom.
    pub fn lines(&self) -> &[UiLine] {
        &self.lines
    }

    /// The pixel size the text was laid out at.
    pub fn font_size(&self) -> f64 {
        self.font_size
    }

    /// Anti-aliased coverage (0..1, row-major) of the glyphs scaled by `scale` (pixels per text
    /// pixel), in the `w × h` window whose top-left corner is at `origin` in scaled pixels from the
    /// text's top-left corner. Each side is clamped to [`MAX_COVERAGE_SIDE`] (the mask's `width`
    /// and `height` say what was filled); a `scale` that is not a positive number is 1. Glyphs that
    /// cannot reach the window are not outlined.
    pub fn coverage(&self, scale: f64, origin: [i64; 2], w: u32, h: u32) -> effectcraft_raster::Mask {
        let (w, h) = (w.min(MAX_COVERAGE_SIDE), h.min(MAX_COVERAGE_SIDE));
        let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
        // Glyph ink rarely strays more than an em or two from its origin.
        let reach = self.font_size * 3.0;
        let (x0, y0) = (origin[0] as f64 / scale - reach, origin[1] as f64 / scale - reach);
        let (x1, y1) = (x0 + f64::from(w) / scale + 2.0 * reach, y0 + f64::from(h) / scale + 2.0 * reach);
        let mut paths = Vec::new();
        for (l, &(dx, dy)) in self.layout.lines.iter().zip(&self.shifts) {
            for g in self.layout.glyphs.get(l.glyphs.clone()).unwrap_or_default() {
                let (gx, gy) = (f64::from(g.x) + dx, f64::from(g.y) + dy);
                if gx < x0 || gx > x1 || gy < y0 || gy > y1 {
                    continue;
                }
                paths.push(effectcraft_text::kurbo::Affine::translate((gx, gy)) * glyph_outline(g));
            }
        }
        let m = Mat3([[scale, 0.0, -origin[0] as f64], [0.0, scale, -origin[1] as f64], [0.0, 0.0, 1.0]]);
        effectcraft_path::fill_coverage(&paths, &m, w, h, effectcraft_path::FillRule::NonZero)
    }
}

/// The leftmost and rightmost caret stop of a line (its trailing spaces included).
fn caret_span(l: &Line) -> (f32, f32) {
    l.carets.iter().fold((f32::MAX, f32::MIN), |(lo, hi), c| (lo.min(c.1), hi.max(c.1)))
}

/// `measureString`: the `[width, height]` of `text` as [`UiText::layout`] lays it out.
pub fn measure_text(text: &str, font: UiFont, size: f64, wrap: Option<f64>) -> Result<[f64; 2], UiTextError> {
    UiText::layout(text, font, size, wrap).map(|t| t.size())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_fonts_resolve_to_bundled_faces() {
        for (family, style, want) in [
            ("dialog", "REGULAR", UiFont::Regular),
            ("palette", "BOLD", UiFont::Bold),
            ("Tahoma", "1", UiFont::Bold),
            ("Arial", "ITALIC", UiFont::Italic),
            ("", "2", UiFont::Italic),
            ("dialog", "BOLDITALIC", UiFont::Bold),
            ("dialog", "3", UiFont::Bold),
            ("Courier New", "BOLD", UiFont::Mono),
            ("Consolas", "", UiFont::Mono),
            ("anything", "[object Object]", UiFont::Regular),
        ] {
            assert_eq!(UiFont::resolve(family, style), want, "{family} {style}");
        }
        for font in [UiFont::Regular, UiFont::Bold, UiFont::Italic, UiFont::Mono] {
            let (family, style) = font.family_style();
            let r = fonts::resolve(family, style);
            assert!(!r.missing && !r.synth_bold && !r.synth_italic, "{font:?} {r:?}");
            assert_eq!(fonts::face(r.face).info.origin, "bundled", "{font:?}");
        }
    }

    /// The limit counts UTF-16 code units, like the script's `length`, and is an error, not a cut.
    #[test]
    fn text_past_the_limit_is_an_error_not_a_shorter_string() {
        let ok = |s: &str| UiText::layout(s, UiFont::Regular, 12.0, None);
        assert!(ok(&"a".repeat(MAX_UI_TEXT_UNITS)).is_ok());
        assert_eq!(ok(&"a".repeat(MAX_UI_TEXT_UNITS + 1)).err(), Some(UiTextError::TooLong { units: MAX_UI_TEXT_UNITS + 1 }));
        // Three bytes but one unit each: within the limit although longer in bytes.
        assert!(ok(&"\u{4e2d}".repeat(MAX_UI_TEXT_UNITS)).is_ok());
        // Astral characters are two units each.
        assert!(ok(&"\u{1f600}".repeat(MAX_UI_TEXT_UNITS / 2)).is_ok());
        assert!(ok(&"\u{1f600}".repeat(MAX_UI_TEXT_UNITS / 2 + 1)).is_err());
        assert!(measure_text(&"a".repeat(MAX_UI_TEXT_UNITS + 1), UiFont::Mono, 12.0, Some(30.0)).is_err());
    }

    #[test]
    fn rows_stack_and_line_breaks_add_rows() {
        let t = UiText::layout("ab\n\nWWWW  \n", UiFont::Regular, 12.5, None).unwrap();
        let rows = t.lines();
        assert_eq!(rows.len(), 4, "{rows:?}");
        assert!(rows.iter().all(|r| r.height == rows[0].height), "Latin rows are all as tall as the face");
        let mut top = 0.0;
        for r in rows {
            assert_eq!(r.top, top);
            assert!(r.baseline > r.top && r.baseline < r.top + r.height);
            top += r.height;
        }
        assert_eq!(t.size()[1], top);
        assert_eq!(rows[1].width, 0.0, "a blank line is a row with no width");
        assert_eq!(rows[3].width, 0.0, "a trailing line break adds an empty row");
        let bare = UiText::layout("WWWW", UiFont::Regular, 12.5, None).unwrap().size()[0];
        assert!(rows[2].width > bare, "a paragraph's trailing spaces are measured: {} vs {bare}", rows[2].width);
        assert_eq!(t.size()[0], rows[2].width);
    }

    /// Rows break where a greedy fill of the words, each measured on its own line, says they do;
    /// repeated spaces hang at the end of a row; a word wider than the bounds is not split.
    #[test]
    fn wrapping_breaks_where_the_words_stop_fitting() {
        let width = |s: &str| UiText::layout(s, UiFont::Regular, 12.0, None).unwrap().size()[0];
        let text = "alpha  beta gamma    delta epsilonzetaetathetaiota kappa";
        // Words as byte ranges: a row runs from its first word's start to its last word's end,
        // with the spaces between them as written.
        let words: Vec<(usize, usize)> = text
            .char_indices()
            .filter(|&(i, c)| c != ' ' && (i == 0 || text.as_bytes()[i - 1] == b' '))
            .map(|(i, _)| (i, text[i..].find(' ').map_or(text.len(), |n| i + n)))
            .collect();
        for wrap in [40.0, 70.0, 95.0, 130.0] {
            let mut rows: Vec<(usize, usize)> = vec![];
            for &(a, b) in &words {
                match rows.last_mut() {
                    Some(r) if width(&text[r.0..b]) <= wrap => r.1 = b,
                    _ => rows.push((a, b)),
                }
            }
            let rows: Vec<&str> = rows.iter().map(|&(a, b)| &text[a..b]).collect();
            let t = UiText::layout(text, UiFont::Regular, 12.0, Some(wrap)).unwrap();
            let got: Vec<&str> = t.lines().iter().map(|l| text[l.range.clone()].trim_end_matches(' ')).collect();
            assert_eq!(got, rows, "at {wrap}");
            for (l, r) in t.lines().iter().zip(&rows) {
                assert!((l.width - width(r)).abs() < 1e-3, "at {wrap}: row {r:?} is {} wide, alone {}", l.width, width(r));
            }
            let long = width("epsilonzetaetathetaiota");
            assert_eq!(t.size()[0], t.lines().iter().map(|l| l.width).fold(0.0, f64::max));
            if long > wrap {
                assert!(t.size()[0] >= long, "the unbroken word overflows the bounds at {wrap}");
            }
        }
        // Ideographs break between characters.
        let cjk = "\u{6f22}\u{5b57}\u{6f22}\u{5b57}\u{6f22}\u{5b57}";
        let one = width("\u{6f22}");
        let t = UiText::layout(cjk, UiFont::Regular, 12.0, Some(one * 2.5)).unwrap();
        assert_eq!(t.lines().len(), 3, "{:?}", t.lines());
        assert!(t.lines().iter().all(|l| l.width <= one * 2.5 + 1e-3));
    }

    /// The painted glyphs are the measured ones: their ink sits in the measured rows (an em of
    /// slack for overhang), a missing glyph paints its box, and spaces paint nothing.
    #[test]
    fn coverage_paints_the_measured_rows() {
        for (text, font) in [
            ("Wildly fine", UiFont::Regular),
            ("\u{6f22}\u{5b57}\u{304b}\u{306a} \u{2714}\u{25a3}\u{25b8}", UiFont::Bold),
            ("\u{645}\u{631}\u{62d}\u{628}\u{627} abc 123", UiFont::Italic),
            ("e\u{301}\u{915}\u{93f}\u{924} \u{5e9}\u{5dc}\u{5d5}\u{5dd}", UiFont::Mono),
            ("\u{378}\u{e000}", UiFont::Regular),
        ] {
            for (size, scale) in [(12.5, 1.25), (9.0, 1.5), (20.0, 2.0)] {
                let t = UiText::layout(text, font, size, None).unwrap();
                let [w, h] = t.size();
                let pad = (size * scale).ceil() as i64;
                let (cw, ch) = ((w * scale).ceil() as i64 + 2 * pad, (h * scale).ceil() as i64 + 2 * pad);
                let cov = t.coverage(scale, [-pad, -pad], cw as u32, ch as u32);
                let ink: Vec<(i64, i64)> = (0..ch).flat_map(|y| (0..cw).map(move |x| (x, y))).filter(|&(x, y)| cov.data[(y * cw + x) as usize] > 0.0).collect();
                assert!(!ink.is_empty(), "{text:?} paints nothing");
                let (x0, x1) = ink.iter().fold((i64::MAX, i64::MIN), |(a, b), p| (a.min(p.0), b.max(p.0)));
                let (y0, y1) = ink.iter().fold((i64::MAX, i64::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
                let slack = size * scale * 0.5;
                let px = |v: i64| (v - pad) as f64;
                assert!(
                    px(x0) >= -slack && px(x1) <= w * scale + slack,
                    "{text:?} {font:?} {size}@{scale}: ink x {}..{} vs width {}",
                    px(x0),
                    px(x1),
                    w * scale
                );
                assert!(
                    px(y0) >= -slack && px(y1) <= h * scale + slack,
                    "{text:?} {font:?} {size}@{scale}: ink y {}..{} vs height {}",
                    px(y0),
                    px(y1),
                    h * scale
                );
            }
        }
        let blank = UiText::layout("   ", UiFont::Regular, 12.0, None).unwrap();
        assert!(blank.size()[0] > 0.0);
        assert!(blank.coverage(1.0, [0, 0], 40, 20).data.iter().all(|&c| c == 0.0));
        // A larger window is filled up to the limit, and the mask says so.
        let m = blank.coverage(f64::NAN, [0, 0], 5000, 3);
        assert_eq!((m.width, m.height, m.data.len()), (MAX_COVERAGE_SIDE, 3, MAX_COVERAGE_SIDE as usize * 3));
    }
}
