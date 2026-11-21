//! ScriptUI text metrics: `measureString` measures the face `drawString` paints with (#3782).

use effectcraft_engine::scriptui::{DrawOp, MAX_UI_TEXT_UNITS, UiFont, measure_text};
use serde_json::{Value, json};

use super::ok;

fn measure(s: &mut effectcraft_engine::Session, args: &str) -> [f64; 2] {
    let code = format!("var g = new Window('palette', 'M').add('group').graphics; var d = g.measureString({args}); [d.width, d.height, d[0], d[1]]");
    let r = ok(s, &code).result;
    let n = |i: usize| r.get(i).and_then(Value::as_f64).unwrap_or(f64::NAN);
    assert_eq!((n(0), n(1)), (n(2), n(3)), "width/height alias [0]/[1] for {args}");
    [n(0), n(1)]
}

fn native(text: &str, font: UiFont, size: f64, wrap: Option<f64>) -> [f64; 2] {
    measure_text(text, font, size, wrap).unwrap().map(f64::ceil)
}

#[test]
fn narrow_and_wide_glyphs_measure_differently() {
    let mut s = super::session();
    let f = "ScriptUI.newFont('dialog', 'REGULAR', 12)";
    let narrow = measure(&mut s, &format!("'iiiiiiii', {f}"));
    let wide = measure(&mut s, &format!("'WWWWWWWW', {f}"));
    assert!(wide[0] > narrow[0] * 2.0, "narrow {narrow:?} wide {wide:?}");
    assert_eq!(narrow, native("iiiiiiii", UiFont::Regular, 12.0, None));
    assert_eq!(wide, native("WWWWWWWW", UiFont::Regular, 12.0, None));
}

#[test]
fn measure_string_uses_the_resolved_family_style_and_size() {
    let mut s = super::session();
    for (font, face, size) in [
        ("ScriptUI.newFont('dialog', 'BOLD', 20)", UiFont::Bold, 20.0),
        ("ScriptUI.newFont('palette', ScriptUI.FontStyle.ITALIC, 15)", UiFont::Italic, 15.0),
        ("ScriptUI.newFont('Courier New', 'REGULAR', 11)", UiFont::Mono, 11.0),
        ("ScriptUI.newFont('Tahoma', 'BOLDITALIC', 9)", UiFont::Bold, 9.0),
    ] {
        assert_eq!(measure(&mut s, &format!("'Wildly fine 0123', {font}")), native("Wildly fine 0123", face, size, None), "{font}");
    }
    // The graphics object's own font when none is given.
    let code = "var g = new Window('palette', 'M').add('group').graphics; g.font = ScriptUI.newFont('dialog', 'BOLD', 18); var d = g.measureString('Hello'); [d.width, d.height]";
    let [w, h] = native("Hello", UiFont::Bold, 18.0, None);
    assert_eq!(ok(&mut s, code).result, json!([w as i64, h as i64]));
    let regular = measure(&mut s, "'Hello', ScriptUI.newFont('dialog', 'REGULAR', 18)");
    assert!(native("Hello", UiFont::Bold, 18.0, None)[0] > regular[0]);
}

#[test]
fn lines_wrapping_and_unicode_follow_the_native_layout() {
    let mut s = super::session();
    let f = "ScriptUI.newFont('dialog', 'REGULAR', 12)";
    let one = measure(&mut s, &format!("'ab', {f}"));
    let three = measure(&mut s, &format!("'ab\\nWWWW\\n', {f}"));
    assert_eq!(three, native("ab\nWWWW\n", UiFont::Regular, 12.0, None));
    assert_eq!(three[1], one[1] * 3.0, "an explicit newline adds a row, a trailing one an empty row");
    let text = "the quick brown fox jumps over the lazy dog";
    let wrapped = measure(&mut s, &format!("'{text}', {f}, 80"));
    assert_eq!(wrapped, native(text, UiFont::Regular, 12.0, Some(80.0)));
    assert!(wrapped[0] <= 80.0 && wrapped[1] >= one[1] * 3.0, "{wrapped:?}");
    // A bounds width the text already fits in changes nothing.
    assert_eq!(measure(&mut s, &format!("'{text}', {f}, 4000")), measure(&mut s, &format!("'{text}', {f}")));
    // Combining marks take no extra advance; precomposed and decomposed forms agree.
    assert_eq!(measure(&mut s, &format!("'e\\u0301e\\u0301', {f}")), measure(&mut s, &format!("'\\u00e9\\u00e9', {f}")));
    let cyrillic = measure(&mut s, &format!("'\\u0416\\u0416\\u0416', {f}"));
    assert_eq!(cyrillic, native("ЖЖЖ", UiFont::Regular, 12.0, None));
    assert!(cyrillic[0] > measure(&mut s, &format!("'\\u0456\\u0456\\u0456', {f}"))[0]);
}

#[test]
fn hostile_measure_arguments_stay_finite() {
    let mut s = super::session();
    for args in [
        "''",
        "'abc', null",
        "'abc', ScriptUI.newFont('dialog', 'REGULAR', NaN)",
        "'abc', ScriptUI.newFont('dialog', 'REGULAR', -4)",
        "'abc', ScriptUI.newFont('dialog', 'REGULAR', 1e300)",
        "'abc', ScriptUI.newFont(undefined, {}, '12')",
        "'abc def', undefined, 0",
        "'abc def', undefined, -5",
        "'abc def', undefined, NaN",
        "'abc def', undefined, Infinity",
        "'abc def', undefined, 1e300",
        "undefined",
        "new Array(16384).join('W\\u0301\\u4e2d\\n')",
    ] {
        let [w, h] = measure(&mut s, args);
        assert!(w.is_finite() && h.is_finite() && w >= 0.0 && h >= 0.0, "{args}: {w} {h}");
    }
    assert_eq!(measure(&mut s, "''")[0], 0.0);
}

#[test]
fn draw_string_records_the_face_measure_string_measured() {
    let mut s = super::session();
    let code = r#"
var w = new Window("palette", "Draw");
var c = w.add("group");
c.preferredSize = [100, 50];
c.onDraw = function () {
  var g = this.graphics;
  g.drawString("A", null, 0, 0, ScriptUI.newFont("Consolas", "REGULAR", 13));
  g.drawString("B", null, 0, 0, ScriptUI.newFont("dialog", "ITALIC", 13));
  g.font = ScriptUI.newFont("dialog", ScriptUI.FontStyle.BOLD, 16);
  g.drawString("C", null, 0, 0);
};
w.show();
"#;
    ok(&mut s, code);
    let fonts: Vec<(UiFont, f64)> = s.script_ui.windows[0].root.children[0]
        .draw
        .iter()
        .filter_map(|op| match op {
            DrawOp::Text { font, size, .. } => Some((*font, *size)),
            _ => None,
        })
        .collect();
    assert_eq!(fonts, [(UiFont::Mono, 13.0), (UiFont::Italic, 13.0), (UiFont::Bold, 16.0)]);
}

/// Text past the limit is refused by measureString and drawString alike, never measured or drawn
/// in part; text at the limit is measured and drawn (#3782).
#[test]
fn measure_and_draw_refuse_the_same_overlong_text() {
    let mut s = super::session();
    // onDraw runs when the window is painted, after the script: it draws its own error message.
    let code = format!(
        r#"
var w = new Window("palette", "Long");
var c = w.add("group");
c.preferredSize = [100, 40];
var g = c.graphics;
var at = new Array({max} + 1).join("W"), over = at + "W";
function tryIt(f) {{ try {{ f(); return "ok"; }} catch (e) {{ return e.name + ": " + e.message; }} }}
c.onDraw = function () {{
  this.graphics.drawString(at, null, 0, 0);
  this.graphics.drawString(tryIt(function () {{ c.graphics.drawString(over, null, 0, 0); }}), null, 0, 0);
}};
writeLn(at.length);
writeLn(tryIt(function () {{ g.measureString(at); }}));
writeLn(tryIt(function () {{ g.measureString(over); }}));
w.show();
"#,
        max = MAX_UI_TEXT_UNITS
    );
    let out = ok(&mut s, &code).output;
    let msg = format!("RangeError: ScriptUI text is {} characters long; at most {MAX_UI_TEXT_UNITS} can be measured or drawn", MAX_UI_TEXT_UNITS + 1);
    assert_eq!(out, [MAX_UI_TEXT_UNITS.to_string(), "ok".into(), msg.clone()]);
    let texts: Vec<String> = s.script_ui.windows[0].root.children[0]
        .draw
        .iter()
        .filter_map(|op| match op {
            DrawOp::Text { text, .. } => Some(if text.len() > 100 { format!("{} chars", text.len()) } else { text.clone() }),
            _ => None,
        })
        .collect();
    assert_eq!(texts, [format!("{MAX_UI_TEXT_UNITS} chars"), msg], "the text within the limit is drawn, the longer text refused");
}

/// The #3782 report: narrow and wide strings measured the same width.
#[test]
fn original_report_measures_narrow_and_wide_apart() {
    let mut s = super::session();
    let code = r#"
var w = new Window("palette", "Original metric probe");
var g = w.add("statictext", undefined, "probe").graphics;
var f = ScriptUI.newFont("dialog", "REGULAR", 12);
var narrow = g.measureString("iiiiiiii", f);
var wide = g.measureString("WWWWWWWW", f);
writeLn(JSON.stringify({narrow: narrow.width, wide: wide.width, sameWidth: narrow.width === wide.width}));
"#;
    let out = ok(&mut s, code).output;
    let [n, w] = [native("iiiiiiii", UiFont::Regular, 12.0, None)[0], native("WWWWWWWW", UiFont::Regular, 12.0, None)[0]];
    assert_eq!(out, [format!(r#"{{"narrow":{n},"wide":{w},"sameWidth":false}}"#)]);
}
