//! Colour contrast of the theme tokens (docs/gaps.md gap 25), separate from the AccessKit audit:
//! the WCAG 2 contrast ratio of the text, accent and focus colours against the backgrounds they
//! are drawn on, for every theme. Text needs 4.5:1, the focus ring and control borders 3:1.
//!
//! Pairs that fall short today are listed in [`KNOWN_BELOW`] with the ratio measured when this was
//! written. The list is a ratchet: a pair must not get worse, and a pair that reaches its target
//! must leave the list. This checks the tokens, not what a panel really draws with them (text on
//! a label colour, an image, a disabled control) and not colour-blindness.

use effectcraft_ui_egui::theme::{ThemeKind, Tokens};
use egui::Color32;

/// WCAG 2 relative luminance of an sRGB colour.
fn luminance(c: Color32) -> f64 {
    let lin = |v: u8| {
        let v = f64::from(v) / 255.0;
        if v <= 0.039_28 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

/// WCAG 2 contrast ratio of two opaque colours.
fn ratio(a: Color32, b: Color32) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// (name, foreground, background, minimum ratio)
fn pairs(t: &Tokens) -> Vec<(&'static str, Color32, Color32, f64)> {
    vec![
        ("text on panel", t.text, t.panel_bg, 4.5),
        ("text on field", t.text, t.field_bg, 4.5),
        ("text on selected row", t.text, t.row_selected, 4.5),
        ("text on row", t.text, t.row, 4.5),
        ("dim text on panel", t.text_dim, t.panel_bg, 4.5),
        ("faint text on panel", t.text_faint, t.panel_bg, 4.5),
        ("active tab text on panel", t.tab_text_active, t.panel_bg, 4.5),
        ("tab text on header", t.tab_text, t.header_bg, 4.5),
        ("hot text on panel", t.hot_text, t.panel_bg, 4.5),
        ("white on accent (primary button)", Color32::WHITE, t.accent, 4.5),
        ("danger text on panel", t.danger, t.panel_bg, 4.5),
        ("warning text on panel", t.warning, t.panel_bg, 4.5),
        ("accent (focus ring) on panel", t.accent, t.panel_bg, 3.0),
        ("focus colour on panel", t.focus, t.panel_bg, 3.0),
        ("field border on panel", t.field_border, t.panel_bg, 3.0),
        ("icon on panel", t.icon, t.panel_bg, 3.0),
    ]
}

/// (theme, pair, ratio measured when written). Ratchet: never below the number; remove the entry
/// when the pair reaches its target.
const KNOWN_BELOW: &[(&str, &str, f64)] = &[
    ("dark", "white on accent (primary button)", 3.46),
    ("dark", "danger text on panel", 3.90),
    ("dark", "field border on panel", 1.38),
    ("darker", "white on accent (primary button)", 3.46),
    ("darker", "danger text on panel", 4.37),
    ("darker", "field border on panel", 1.55),
    ("light", "faint text on panel", 4.46),
    ("light", "white on accent (primary button)", 3.46),
    ("light", "danger text on panel", 3.23),
    ("light", "warning text on panel", 1.85),
    ("light", "accent (focus ring) on panel", 2.78),
    ("light", "focus colour on panel", 2.78),
    ("light", "field border on panel", 1.86),
];

#[test]
fn print_and_hold_the_theme_contrast() {
    let mut failures = vec![];
    for kind in [ThemeKind::Dark, ThemeKind::Darker, ThemeKind::Light] {
        let t = Tokens::for_kind(kind);
        for (name, fg, bg, min) in pairs(&t) {
            let r = ratio(fg, bg);
            let known = KNOWN_BELOW.iter().find(|(k, n, _)| *k == kind.name() && *n == name);
            println!("{:<7} {name:<36} {r:>5.2}:1 (needs {min})", kind.name());
            match known {
                Some((_, _, floor)) => {
                    if r + 0.005 < *floor {
                        failures.push(format!("{} / {name}: {r:.2}:1 is worse than the known {floor:.2}:1", kind.name()));
                    }
                    if r >= min {
                        failures.push(format!("{} / {name}: {r:.2}:1 now reaches {min}:1; remove it from KNOWN_BELOW", kind.name()));
                    }
                }
                None if r < min => failures.push(format!("{} / {name}: {r:.2}:1 is below {min}:1", kind.name())),
                None => {}
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
