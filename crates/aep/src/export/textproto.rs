//! The text document a text layer starts from, built in code (`docs/aep-format.md` 3.10): the
//! smallest dictionary After Effects reads back unchanged (read-backs t2-*, t3-*). `text::build_doc`
//! then writes the text, the runs, the fonts and the box frame into it.
//!
//! What stays and why: the font list (`/0/1`, names only), the style and paragraph sheets with
//! their name only (`/0/5`, `/0/6`; without them the layer is skipped), one text frame (`/0/8`: a
//! point text's, or a box text's with its outline), the document settings (`/1/0`: the line-break
//! classes of seven characters, the superscript and subscript size and offset, the small caps
//! size; without them, or with `/1/0/0` cut down, After Effects refuses or crashes), the composer
//! (`/1/5`; without it After Effects rewrites the document on open) and one run with one
//! paragraph and one style (`/1/1`) and an empty layout.

use crate::export::textdoc::{TNode, boolean, int, number, string};

fn dict(kv: Vec<(&str, TNode)>) -> TNode {
    TNode::Dict(kv.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

fn name(n: &str) -> TNode {
    TNode::Atom(format!("/{n}").into_bytes())
}

fn numbers(v: &[f64]) -> TNode {
    TNode::Array(v.iter().map(|x| number(*x)).collect())
}

/// The fonts of a new document (the first is the one the runs use until `build_doc` sets theirs).
const FONTS: [&str; 4] = ["TimesNewRomanPSMT", "Myriad-Roman", "MyriadPro-Regular", "AdobeInvisFont"];

/// The line-break class of the characters the text engine treats specially: space, carriage
/// return, tab, paragraph separator, end of text, ideographic space, soft hyphen.
const BREAK_CLASSES: [(char, &str); 7] = [(' ', "1"), ('\r', "6"), ('\t', "0"), ('\u{2029}', "5"), ('\u{3}', "5"), ('\u{3000}', "1"), ('\u{ad}', "3")];

/// A sheet list with one sheet named `Normal RGB`, used by every run (`/1 [ << /0 0 >> ]`).
fn sheets() -> TNode {
    dict(vec![("0", TNode::Array(vec![dict(vec![("0", dict(vec![("0", string("Normal RGB"))]))])])), ("1", TNode::Array(vec![dict(vec![("0", int(0))])]))])
}

/// The text frame: a point text's (no outline, size -1), or a box text's with an outline that
/// `build_doc` replaces with the layer's box.
fn frame(boxed: bool) -> TNode {
    let (kind, size) = if boxed { (Some(1), -2.0) } else { (None, -1.0) };
    let mut settings = vec![("6", numbers(&[size, size])), ("11", dict(vec![("4", int(size as i64)), ("18", number(size)), ("22", number(0.025))]))];
    if let Some(k) = kind {
        settings.insert(0, ("0", int(k)));
    }
    let mut entry = Vec::new();
    if boxed {
        entry.push(("1", dict(vec![("0", numbers(&[0.0; 32]))])));
    }
    entry.push(("2", dict(settings)));
    dict(vec![("0", TNode::Array(vec![dict(vec![("0", dict(entry))])]))])
}

fn paint() -> TNode {
    dict(vec![("99", name("SimplePaint")), ("0", dict(vec![("0", int(1)), ("1", numbers(&[1.0, 1.0, 1.0, 1.0]))]))])
}

/// The run: an empty text with one paragraph and one style run (the keys `build_doc` writes, at
/// the values it treats as unset) and an empty layout.
fn run() -> TNode {
    let para = dict(vec![
        ("0", int(0)),
        ("1", number(0.0)),
        ("2", number(0.0)),
        ("3", number(0.0)),
        ("4", number(0.0)),
        ("5", number(0.0)),
        ("21", boolean(false)),
        ("29", boolean(false)),
        ("33", int(0)),
    ]);
    let style = dict(vec![
        ("0", int(0)),
        ("1", number(12.0)),
        ("2", boolean(false)),
        ("3", boolean(false)),
        ("4", boolean(true)),
        ("5", number(0.01)),
        ("6", number(1.0)),
        ("7", number(1.0)),
        ("8", int(0)),
        ("9", number(0.0)),
        ("11", int(1)),
        ("12", int(0)),
        ("13", int(0)),
        ("18", boolean(false)),
        ("36", number(0.0)),
        ("53", paint()),
        ("54", paint()),
        ("56", boolean(true)),
        ("57", boolean(false)),
        ("58", boolean(true)),
        ("63", number(1.0)),
    ]);
    // Each run names its sheet (`/0`) and the other sheet's index (0), in key order.
    let entry = |kv: Vec<(&str, TNode)>| {
        let mut inner = vec![("0", string(""))];
        inner.extend(kv);
        TNode::Array(vec![dict(vec![("0", dict(vec![("0", dict(inner))])), ("1", int(0))])])
    };
    let text = dict(vec![
        ("0", string("")),
        ("5", dict(vec![("0", entry(vec![("5", para), ("6", int(0))]))])),
        ("6", dict(vec![("0", entry(vec![("5", int(0)), ("6", style)]))])),
    ]);
    let layout = dict(vec![("0", TNode::Array(vec![dict(vec![("0", int(0))])])), ("2", TNode::Array(Vec::new()))]);
    dict(vec![("0", text), ("1", layout)])
}

/// A new document, for a box text when `boxed`.
pub(crate) fn document(boxed: bool) -> TNode {
    let fonts = FONTS.iter().map(|f| dict(vec![("0", dict(vec![("99", name("CoolTypeFont")), ("0", dict(vec![("0", string(f))]))]))])).collect();
    let resources = dict(vec![("1", dict(vec![("0", TNode::Array(fonts))])), ("5", sheets()), ("6", sheets()), ("8", frame(boxed))]);
    let classes = BREAK_CLASSES.iter().map(|(c, class)| dict(vec![("0", string(&c.to_string())), ("1", string(class))])).collect();
    let settings = dict(vec![
        ("0", dict(vec![("0", int(3)), ("1", TNode::Array(classes))])),
        ("1", int(0)),
        ("2", int(0)),
        // Superscript size and offset, subscript size and offset, small caps size.
        ("3", number(0.583)),
        ("4", number(0.333)),
        ("5", number(0.583)),
        ("6", number(0.333)),
        ("7", number(0.7)),
        ("8", boolean(true)),
        ("16", boolean(false)),
        ("17", int(0)),
    ]);
    let composer = TNode::Array(vec![dict(vec![
        ("0", int(0)),
        ("1", dict(vec![("0", int(7)), ("1", int(10)), ("2", int(0)), ("3", string("DVA"))])),
        ("2", string_bits("10000001000001010001100000000011101000000000000110000000")),
        ("3", string_bits("00110")),
        ("4", TNode::Array(vec![dict(vec![("0", int(0)), ("1", string("Version 7.12"))])])),
    ])]);
    let doc_body = dict(vec![("0", settings), ("5", composer), ("1", TNode::Array(vec![run()]))]);
    dict(vec![("0", resources), ("1", doc_body)])
}

/// A string of bits as the composer stores its flags: plain ASCII between parentheses.
fn string_bits(bits: &str) -> TNode {
    TNode::Atom(format!("({bits})").into_bytes())
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_document_parses_back_and_has_the_paths_build_doc_writes() {
        for boxed in [false, true] {
            let doc = super::document(boxed);
            let bytes = crate::export::textdoc::write(&doc);
            assert_eq!(crate::export::textdoc::parse(&bytes), Some(doc.clone()));
            for path in ["0/1/0", "1/1[0]/0/5/0[0]/0/0/5", "1/1[0]/0/6/0[0]/0/0/6/53/0", "1/1[0]/1"] {
                assert!(doc.path(path).is_some(), "{path}");
            }
            assert_eq!(doc.path("0/8/0[0]/0/1").is_some(), boxed);
        }
    }
}

#[cfg(test)]
mod needed {
    /// The parts After Effects skips the layer without, or crashes on (read-backs t2-*, t3-*).
    #[test]
    fn the_document_keeps_what_after_effects_needs() {
        for boxed in [false, true] {
            let doc = super::document(boxed);
            for path in ["0/5", "0/6", "0/8", "1/0", "1/5", "1/0/0/0"] {
                assert!(doc.path(path).is_some(), "{path}");
            }
            match doc.path("1/0/0/1") {
                Some(crate::export::textdoc::TNode::Array(classes)) => assert_eq!(classes.len(), 7),
                other => panic!("line-break classes: {other:?}"),
            }
            assert!(doc.path("1/1[0]/0/5/0[0]/0/0/6").is_some() && doc.path("1/1[0]/0/6/0[0]/0/0/5").is_some());
        }
    }
}
