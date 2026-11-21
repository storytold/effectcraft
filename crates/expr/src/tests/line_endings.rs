//! After Effects stores expressions with `\r` line breaks. A `// comment` ends at one, as at any
//! JavaScript line terminator: it used to run to the end of the text and swallow every line after
//! it, so the expression silently returned what its first line computed.

use super::*;

#[test]
fn a_line_comment_ends_at_a_carriage_return() {
    let f = fx();
    // A posterize-time Offset as it comes from a project file (Rotation keys: 0 at 0 s, 90 at 1 s).
    let posterized =
        "fps = 12.5; // Set your desired frame rate\rposterizeTime(fps); // evaluate at the posterized time\rvalueAtTime(Math.floor(time * fps) / fps)";
    let lf = posterized.replace('\r', "\n");
    assert_close!(f.num(posterized, 0.5), f.num(&lf, 0.5));
    assert_close!(f.num("// a header line\r7", 0.0), 7.0);
    assert_close!(f.num("x = 3; // note\r\nx + 4", 0.0), 7.0);
    assert_close!(f.num("x = 3; // note\u{2028}x + 5", 0.0), 8.0);
}

#[test]
fn errors_keep_their_line_number_with_carriage_returns() {
    let f = fx();
    let e = f.ev(f.a, ROT, "x = 1; // one\ry = 2; // two\rundefinedThing + 1", 0.0).unwrap_err();
    assert!(e.contains("line 3"), "{e}");
}

#[test]
fn unicode_line_separators_stay_inside_strings_and_templates() {
    // Review of #3552: U+2028 / U+2029 are kept by JavaScript strings and templates.
    let f = fx();
    assert_close!(f.num("'\u{2028}'.charCodeAt(0)", 0.0), 8232.0);
    assert_close!(f.num("`\u{2029}`.charCodeAt(0)", 0.0), 8233.0);
    // ...and still end a line comment.
    assert_close!(f.num("x = 3; // note\u{2028}x + 5", 0.0), 8.0);
}

#[test]
fn an_error_after_a_unicode_line_separator_keeps_its_line() {
    // Review of #3552: U+2028 / U+2029 count as line breaks for the error's line too.
    let f = fx();
    let e = f.ev(f.a, ROT, "x = 1;\u{2028}y = 2;\u{2029}undefinedThing + 1", 0.0).unwrap_err();
    assert!(e.contains("line 3"), "{e}");
}
