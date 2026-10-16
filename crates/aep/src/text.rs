//! Convert an exported After Effects TextDocument into editable Source Text.

use effectcraft_keyframe::text_doc::{DigitSet, FontFileLink};
use effectcraft_keyframe::{BaselineOption, CharStyle, Composer, Direction, Justify, Kerning, ParaStyle, TextDoc, Value};
use serde_json::Value as Json;

/// Host-verified AE missing-font substitutes. Other names/versions stay unresolved.
fn verified_render_font(requested: &str, version: Option<&str>) -> Option<&'static str> {
    (version == Some("Version 3.019;git-0a5106e0b") && (requested.eq_ignore_ascii_case("Inter-Light") || requested.eq_ignore_ascii_case("Inter-Black")))
        .then_some("MyriadPro-Regular")
}

fn font_file(j: &Json) -> Option<FontFileLink> {
    serde_json::from_value(j.get("font_file")?.clone()).ok()
}

fn number(j: &Json, key: &str) -> Option<f64> {
    j.get(key)?.as_f64().filter(|n| n.is_finite())
}

fn scale(j: &Json, key: &str) -> Option<f64> {
    number(j, key).filter(|v| *v > 0.0).map(|v| v * 100.0).filter(|v| v.is_finite())
}

fn pair(j: &Json, key: &str) -> Option<[f64; 2]> {
    let a = j.get(key)?.as_array()?;
    Some([a.first()?.as_f64().filter(|n| n.is_finite())?, a.get(1)?.as_f64().filter(|n| n.is_finite())?])
}

fn color(j: &Json, key: &str) -> Option<[f32; 4]> {
    let a = j.get(key)?.as_array()?;
    let channel = |i: usize| a.get(i)?.as_f64().filter(|n| n.is_finite()).map(|n| n.clamp(0.0, 1.0) as f32);
    Some([channel(0)?, channel(1)?, channel(2)?, a.get(3).map_or(Some(1.0), |_| channel(3))?])
}

fn justification(j: &Json) -> Option<Justify> {
    let v = j.get("justification")?;
    if let Some(s) = v.as_str() {
        return Justify::parse(s.trim_start_matches("ParagraphJustification.").trim_end_matches("_JUSTIFY"));
    }
    // ExtendScript ParagraphJustification enum values (7412 means mixed paragraphs).
    Some(match v.as_i64()? {
        7413 => Justify::Left,
        7414 => Justify::Right,
        7415 => Justify::Center,
        7416 => Justify::JustifyLastLeft,
        7417 => Justify::JustifyLastRight,
        7418 => Justify::JustifyLastCenter,
        7419 => Justify::JustifyAll,
        _ => return None,
    })
}

fn character_style(s: &mut CharStyle, j: &Json) {
    if let Some(v) = j.get("font").and_then(Json::as_str) {
        s.font = v.to_owned();
        s.font_version = None;
        s.font_file = None;
    }
    if let Some(v) = j.get("font_version").and_then(Json::as_str) {
        s.font_version = Some(v.to_owned());
    }
    if j.get("font_file").is_some() {
        s.font_file = font_file(j);
    }
    s.render_font = verified_render_font(&s.font, s.font_version.as_deref()).map(str::to_owned);
    if let Some(v) = j.get("font_style").and_then(Json::as_str) {
        s.style = v.to_owned();
    }
    if let Some(v) = number(j, "font_size").filter(|v| *v > 0.0) {
        s.size = v;
    }
    if let Some(v) = color(j, "fill_color") {
        s.fill = v;
    }
    if let Some(v) = color(j, "stroke_color") {
        s.stroke = v;
    }
    if let Some(v) = j.get("apply_fill").and_then(Json::as_bool) {
        s.apply_fill = v;
    }
    if let Some(v) = j.get("apply_stroke").and_then(Json::as_bool) {
        s.apply_stroke = v;
    }
    if let Some(v) = number(j, "stroke_width").filter(|v| *v >= 0.0) {
        s.stroke_width = v;
    }
    if let Some(v) = number(j, "tracking") {
        s.tracking = v;
    }
    if j.get("auto_leading").and_then(Json::as_bool) == Some(true) {
        s.leading = None;
    } else if let Some(v) = number(j, "leading").filter(|v| *v > 0.0) {
        s.leading = Some(v);
    }
    if let Some(v) = number(j, "baseline_shift") {
        s.baseline_shift = v;
    }
    // py-aep exposes these as factors; EffectCraft stores percentages.
    if let Some(v) = scale(j, "horizontal_scale") {
        s.h_scale = v;
    }
    if let Some(v) = scale(j, "vertical_scale") {
        s.v_scale = v;
    }
    if let Some(v) = number(j, "tsume") {
        s.tsume = (v * 100.0).clamp(0.0, 100.0);
    }
    for (key, dst) in [
        ("all_caps", &mut s.all_caps),
        ("small_caps", &mut s.small_caps),
        ("faux_bold", &mut s.faux_bold),
        ("faux_italic", &mut s.faux_italic),
        ("ligature", &mut s.ligatures),
    ] {
        if let Some(v) = j.get(key).and_then(Json::as_bool) {
            *dst = v;
        }
    }
    if let Some(v) = j.get("font_caps_option").and_then(Json::as_i64) {
        match v {
            11012 => {
                s.all_caps = false;
                s.small_caps = false;
                s.opentype.all_small_caps = false;
            }
            11013 => {
                s.all_caps = false;
                s.small_caps = true;
                s.opentype.all_small_caps = false;
            }
            11014 => {
                s.all_caps = true;
                s.small_caps = false;
                s.opentype.all_small_caps = false;
            }
            11015 => {
                s.all_caps = false;
                s.small_caps = false;
                s.opentype.all_small_caps = true;
            }
            _ => {}
        }
    }
    if let Some(v) = j.get("font_baseline_option").and_then(Json::as_i64) {
        s.baseline = match v {
            11213 => BaselineOption::Superscript,
            11214 => BaselineOption::Subscript,
            _ => BaselineOption::Normal,
        };
    }
    if j.get("superscript").and_then(Json::as_bool) == Some(true) {
        s.baseline = BaselineOption::Superscript;
    }
    if j.get("subscript").and_then(Json::as_bool) == Some(true) {
        s.baseline = BaselineOption::Subscript;
    }
    if let Some(v) = j.get("auto_kern_type").and_then(Json::as_i64) {
        s.kerning = match v {
            11412 => Kerning::Manual(0.0),
            11414 => Kerning::Optical,
            _ => Kerning::Metrics,
        };
    }
    if let Some(v) = number(j, "kerning") {
        s.kerning = Kerning::Manual(v);
    }
    if j.get("baseline_direction").and_then(Json::as_i64) == Some(11614) {
        s.tate_chu_yoko = true;
    }
}

fn paragraph_style(p: &mut ParaStyle, j: &Json) {
    if let Some(v) = justification(j) {
        p.justify = v;
    }
    for (key, dst) in [
        ("first_line_indent", &mut p.indent_first),
        ("start_indent", &mut p.indent_left),
        ("end_indent", &mut p.indent_right),
        ("space_before", &mut p.space_before),
        ("space_after", &mut p.space_after),
    ] {
        if let Some(v) = number(j, key) {
            *dst = v;
        }
    }
    if let Some(v) = j.get("direction").and_then(Json::as_i64) {
        p.direction = if v == 10213 { Direction::Rtl } else { Direction::Ltr };
    }
    if let Some(v) = j.get("every_line_composer").and_then(Json::as_bool) {
        p.composer = if v { Composer::EveryLine } else { Composer::SingleLine };
    }
    if let Some(v) = j.get("hanging_roman").and_then(Json::as_bool) {
        p.hanging_punctuation = v;
    }
    if let Some(v) = j.get("auto_hyphenate").and_then(Json::as_bool) {
        p.auto_hyphenate = v;
    }
}

fn run_range(run: &Json, offsets: &[usize]) -> Option<std::ops::Range<usize>> {
    let start = usize::try_from(run.get("start_utf16")?.as_u64()?).ok()?;
    let end = usize::try_from(run.get("end_utf16")?.as_u64()?).ok()?;
    if end < start {
        return None;
    }
    let visible_end = *offsets.last()?;
    if start > visible_end {
        return None;
    }
    // Snap a split surrogate pair outward so a UTF-16 run still styles its character.
    let a = offsets.partition_point(|n| *n <= start).saturating_sub(1);
    let b = offsets.partition_point(|n| *n < end.min(visible_end));
    Some(a..b.min(offsets.len().saturating_sub(1)))
}

pub(crate) fn value(j: &Json) -> Option<Value> {
    let text = j.get("text")?.as_str()?;
    let mut d = TextDoc::plain(&text.replace(['\r', '\u{3}'], "\n"));
    if let Some(v) = j.get("font").and_then(Json::as_str) {
        d.font = v.to_owned();
    }
    if let Some(v) = j.get("font_version").and_then(Json::as_str) {
        d.font_version = Some(v.to_owned());
    }
    d.font_file = font_file(j);
    d.render_font = verified_render_font(&d.font, d.font_version.as_deref()).map(str::to_owned);
    if let Some(v) = j.get("font_style").and_then(Json::as_str) {
        d.style = v.to_owned();
    }
    if let Some(v) = number(j, "font_size").filter(|v| *v > 0.0) {
        d.size = v;
    }
    if let Some(v) = color(j, "fill_color") {
        d.fill = v;
    }
    if let Some(v) = color(j, "stroke_color") {
        d.stroke = v;
    }
    if let Some(v) = j.get("apply_fill").and_then(Json::as_bool) {
        d.apply_fill = v;
    }
    if let Some(v) = j.get("apply_stroke").and_then(Json::as_bool) {
        d.apply_stroke = v;
    }
    if let Some(v) = j.get("stroke_over_fill").and_then(Json::as_bool) {
        d.stroke_over_fill = v;
    }
    if let Some(v) = number(j, "stroke_width").filter(|v| *v >= 0.0) {
        d.stroke_width = v;
    }
    if let Some(v) = justification(j) {
        d.justify = v;
    }
    if let Some(v) = number(j, "tracking") {
        d.tracking = v;
    }
    if j.get("auto_leading").and_then(Json::as_bool) != Some(true) {
        d.leading = number(j, "leading").filter(|v| *v > 0.0);
    }
    if let Some(v) = number(j, "baseline_shift") {
        d.baseline_shift = v;
    }
    if let Some(v) = scale(j, "horizontal_scale") {
        d.h_scale = v;
    }
    if let Some(v) = scale(j, "vertical_scale") {
        d.v_scale = v;
    }
    if j.get("box_text").and_then(Json::as_bool) == Some(true) {
        d.box_size = pair(j, "box_text_size").filter(|v| v[0] > 0.0 && v[1] > 0.0);
        if let Some(v) = pair(j, "box_text_pos") {
            d.box_pos = v;
        }
    }
    for (key, dst) in [("all_caps", &mut d.all_caps), ("small_caps", &mut d.small_caps), ("faux_bold", &mut d.faux_bold), ("faux_italic", &mut d.faux_italic)] {
        if let Some(v) = j.get(key).and_then(Json::as_bool) {
            *dst = v;
        }
    }
    let mut offsets = Vec::with_capacity(d.char_len().saturating_add(1));
    offsets.push(0);
    for ch in d.text.chars() {
        offsets.push(offsets.last().copied().unwrap_or(0usize).saturating_add(ch.len_utf16()));
    }
    if let Some(runs) = j.get("character_runs").and_then(Json::as_array) {
        if let Some(style) = runs.first().and_then(|r| r.get("style")) {
            if let Some(v) = style.get("stroke_over_fill").and_then(Json::as_bool) {
                d.stroke_over_fill = v;
            }
            if let Some(v) = style.get("digit_set").and_then(DigitSet::parse) {
                d.digit_set = v;
            }
        }
        for run in runs {
            if let (Some(r), Some(style)) = (run_range(run, &offsets), run.get("style")) {
                d.apply_style(r, |s| character_style(s, style));
            }
        }
    }
    if let Some(runs) = j.get("kerning_runs").and_then(Json::as_array) {
        for run in runs {
            if let (Some(r), Some(v)) = (run_range(run, &offsets), run.pointer("/raw_style/0").and_then(Json::as_f64))
                && v.is_finite()
            {
                d.apply_style(r, |s| s.kerning = Kerning::Manual(v));
            }
        }
    }
    if let Some(v) = number(j, "leading_edge_kerning") {
        d.apply_style(0..1, |s| s.kerning = Kerning::Manual(v));
    }
    if let Some(runs) = j.get("paragraph_runs").and_then(Json::as_array) {
        for run in runs {
            if let (Some(r), Some(style)) = (run_range(run, &offsets), run.get("style")) {
                d.apply_para(r, |p| paragraph_style(p, style));
            }
        }
    }
    Some(Value::Text(Box::new(d)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_editable_text_and_styles() {
        let j = serde_json::json!({
            "text": "Hi\rthere", "font": "Example Sans", "font_size": 48,
            "fill_color": [0.2, 0.4, 0.6], "stroke_color": [1, 0, 0],
            "apply_fill": true, "apply_stroke": true, "stroke_width": 2.5,
            "justification": 7418, "tracking": 25, "leading": 60,
            "auto_leading": false, "baseline_shift": 3,
            "horizontal_scale": 90, "vertical_scale": 110,
            "box_text": true, "box_text_size": [300, 200], "box_text_pos": [10, 20],
            "all_caps": true, "faux_bold": true
        });
        let Some(Value::Text(d)) = value(&j) else { panic!("expected text") };
        assert_eq!(d.text, "Hi\nthere");
        assert_eq!(d.font, "Example Sans");
        assert_eq!(d.size, 48.0);
        assert_eq!(d.fill, [0.2, 0.4, 0.6, 1.0]);
        assert_eq!(d.justify, Justify::JustifyLastCenter);
        assert_eq!(d.leading, Some(60.0));
        assert_eq!(d.box_size, Some([300.0, 200.0]));
        assert_eq!(d.box_pos, [10.0, 20.0]);
        assert!(d.all_caps && d.faux_bold && d.apply_stroke);
        assert_eq!(d.stroke_width, 2.5);
        assert_eq!(value(&serde_json::json!({"text": 42})), None);
    }

    #[test]
    fn imports_mixed_utf16_character_and_paragraph_runs() {
        let j = serde_json::json!({
            "text": "A😀\rB", "font": "Base", "horizontal_scale": 1.0,
            "character_runs": [
                {"start_utf16": 0, "end_utf16": 1, "style": {"font": "First", "digit_set": 12014, "stroke_over_fill": false}},
                {"start_utf16": 1, "end_utf16": 3, "style": {
                    "font": "Emoji", "font_size": 36, "fill_color": [1, 0, 0],
                    "horizontal_scale": 1.25, "font_caps_option": 11013
                }},
                {"start_utf16": 3, "end_utf16": 5, "style": {"font": "Last"}}
            ],
            "paragraph_runs": [
                {"start_utf16": 0, "end_utf16": 4, "style": {"justification": 7413, "auto_hyphenate": true}},
                {"start_utf16": 4, "end_utf16": 5, "style": {
                    "justification": 7414, "start_indent": 12, "direction": 10213
                }}
            ],
            "kerning_runs": [{"start_utf16": 1, "end_utf16": 3, "raw_style": {"0": 25}}],
            "leading_edge_kerning": -10
        });
        let Some(Value::Text(d)) = value(&j) else { panic!("expected text") };
        assert_eq!(d.text, "A😀\nB");
        assert_eq!(d.h_scale, 100.0);
        assert_eq!(d.digit_set, DigitSet::Hindi);
        assert!(!d.stroke_over_fill);
        assert_eq!(d.style_at(0).font, "First");
        assert_eq!(d.style_at(0).kerning, Kerning::Manual(-10.0));
        assert_eq!(d.style_at(1).font, "Emoji");
        assert_eq!(d.style_at(1).kerning, Kerning::Manual(25.0));
        assert_eq!(d.style_at(1).size, 36.0);
        assert_eq!(d.style_at(1).h_scale, 125.0);
        assert!(d.style_at(1).small_caps);
        assert_eq!(d.style_at(3).font, "Last");
        assert_eq!(d.para(0).justify, Justify::Left);
        assert!(d.para(0).auto_hyphenate);
        assert_eq!(d.para(1).justify, Justify::Right);
        assert!(!d.para(1).auto_hyphenate);
        assert_eq!(d.para(1).indent_left, 12.0);
        assert_eq!(d.para(1).direction, Direction::Rtl);
    }

    #[test]
    fn verified_font_substitute_preserves_requested_name_in_runs() {
        let j = serde_json::json!({
            "text": "ABC", "font": "Inter-Light", "font_version": "Version 3.019;git-0a5106e0b",
            "character_runs": [
                {"start_utf16": 0, "end_utf16": 1, "style": {"font": "Inter-Light", "font_version": "Version 3.019;git-0a5106e0b"}},
                {"start_utf16": 1, "end_utf16": 2, "style": {"font": "Inter-Black", "font_version": "Version 3.019;git-0a5106e0b"}},
                {"start_utf16": 2, "end_utf16": 3, "style": {"font": "Inter-Black", "font_version": "3.3;20b39288a"}}
            ]
        });
        let Some(Value::Text(d)) = value(&j) else { panic!("expected text") };
        assert_eq!(d.style_at(0).font, "Inter-Light");
        assert_eq!(d.style_at(0).render_font.as_deref(), Some("MyriadPro-Regular"));
        assert_eq!(d.style_at(1).font, "Inter-Black");
        assert_eq!(d.style_at(1).render_font.as_deref(), Some("MyriadPro-Regular"));
        assert_eq!(d.style_at(2).font, "Inter-Black");
        assert_eq!(d.style_at(2).render_font, None);
    }

    #[test]
    fn imports_project_local_font_link_without_rewriting_requested_name() {
        let link = serde_json::json!({"path":"/tmp/source/Brand.otf","sha256":"a".repeat(64)});
        let j = serde_json::json!({
            "text":"Brand", "font":"BRSonoma-Regular", "font_version":"Version 1.200",
            "font_file":link.clone(),
            "character_runs":[{"start_utf16":0,"end_utf16":5,"style":{
                "font":"BRSonoma-Regular", "font_version":"Version 1.200", "font_file":link
            }}]
        });
        let Some(Value::Text(d)) = value(&j) else { panic!("expected text") };
        assert_eq!(d.font, "BRSonoma-Regular");
        assert_eq!(d.font_file.as_ref().map(|f| f.path.as_str()), Some("/tmp/source/Brand.otf"));
        assert_eq!(d.style_at(1).font_file, d.font_file);
        assert_eq!(d.render_font, None);
    }
}
