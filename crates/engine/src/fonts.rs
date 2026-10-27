use super::*;

pub fn text_families() -> Vec<String> {
    effectcraft_text::families().into_iter().map(|(f, _)| f).collect()
}

/// The styles a family offers (its own faces, in weight order), for the style menus. A family
/// that isn't installed offers the usual styles, which resolve to Inter's.
pub fn font_styles(family: &str) -> Vec<String> {
    effectcraft_text::families()
        .into_iter()
        .find(|(f, _)| f.eq_ignore_ascii_case(family))
        .map(|(_, st)| st)
        .filter(|st| !st.is_empty())
        .unwrap_or_else(|| ["Regular", "Medium", "SemiBold", "Bold", "Italic"].iter().map(|s| s.to_string()).collect())
}

/// The OpenType feature tags (GSUB / GPOS) of the face a family + style resolves to.
pub fn font_features(family: &str, style: &str) -> Vec<String> {
    effectcraft_text::fonts::face(effectcraft_text::resolve(family, style).face).features()
}

/// One row of the Character panel's font menu.
#[derive(Clone, Debug, PartialEq)]
pub struct FontRow {
    /// The family to apply (empty for the separator after the recent fonts).
    pub family: String,
    /// What the menu shows: the English family name, or the font's own-language name when
    /// Settings ▸ Type ▸ Show Font Names in English is off.
    pub display: String,
    pub recent: bool,
    /// A variable font (the menu's Variable Fonts filter keeps only these).
    pub variable: bool,
}

/// Outline polylines of "Sample" set in `family` at `size` px (baseline at y = 0, y down), for
/// the font menu's preview (Settings ▸ Type ▸ Show Font Preview).
pub fn font_preview(family: &str, size: f64) -> Vec<Vec<[f32; 2]>> {
    use kurbo::PathEl;
    let st = effectcraft_keyframe::text_doc::CharStyle { font: family.to_string(), size, ..Default::default() };
    let mut out = vec![];
    let mut x = 0.0;
    for ch in "Sample".chars() {
        let (path, adv) = effectcraft_text::char_glyph_style(&st, ch);
        let mut cur: Vec<[f32; 2]> = vec![];
        let pt = |p: kurbo::Point| [(p.x + x) as f32, p.y as f32];
        kurbo::flatten(&path, 0.2, |el| match el {
            PathEl::MoveTo(p) => {
                if cur.len() > 1 {
                    out.push(std::mem::take(&mut cur));
                }
                cur = vec![pt(p)];
            }
            PathEl::LineTo(p) => cur.push(pt(p)),
            PathEl::ClosePath => {
                if let Some(f) = cur.first().copied() {
                    cur.push(f);
                }
                if cur.len() > 1 {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => {}
        });
        if cur.len() > 1 {
            out.push(cur);
        }
        x += adv;
    }
    out
}

/// The Character panel's font menu: the recent fonts (Settings ▸ Type ▸ Number of Recent Fonts
/// to Display), a separator, then every family. Each row says whether the family is variable,
/// for the menu's Variable Fonts filter.
pub fn font_menu(prefs: &prefs::Prefs) -> Vec<FontRow> {
    let all = text_families();
    let native = if prefs.type_.font_names_in_english { Default::default() } else { effectcraft_text::fonts::native_families() };
    let variable = effectcraft_text::fonts::variable_families();
    let row = |f: &String, recent: bool| FontRow {
        family: f.clone(),
        display: native.get(f).cloned().unwrap_or_else(|| f.clone()),
        recent,
        variable: variable.contains(f),
    };
    let mut out: Vec<FontRow> = prefs.recent_fonts_shown().iter().filter(|f| all.contains(f)).map(|f| row(f, true)).collect();
    if !out.is_empty() {
        out.push(FontRow { family: String::new(), display: "-".into(), recent: false, variable: false });
    }
    out.extend(all.iter().map(|f| row(f, false)));
    out
}
