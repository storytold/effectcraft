//! The noun a newly created item is named with, per interface language (#2689).
//!
//! A layer name is project data, so the interface passes the noun it draws and the engine numbers
//! and stores it (`カメラ 1`, `コンポ 1`), as the reference application does. A language
//! adds its rows here (and a match arm) and gets its own names; a language without a row keeps
//! the English noun.
//!
//! This is deliberately not part of the interface catalog: `tr("Light")` there is the appearance
//! toggle ("light theme" — 浅色, Clair, Claro), which would name a light layer `浅色 1`.

/// `ja` rows, observed in the installed After Effects 2026 (26.5) Japanese interface.
const JAPANESE: &[(&str, &str)] = &[
    ("Camera", "カメラ"),
    ("Light", "ライト"),
    ("Null", "ヌル"),
    ("Solid", "平面"),
    ("Shape Layer", "シェイプレイヤー"),
    ("Adjustment Layer", "調整レイヤー"),
    ("Comp", "コンポ"),
];

/// The word `english` in the current interface language, or `english` itself.
pub(crate) fn noun(english: &'static str) -> &'static str {
    let rows = match super::ui::current() {
        "ja" => JAPANESE,
        _ => return english,
    };
    rows.iter().find(|(source, _)| *source == english).map_or(english, |(_, word)| word)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every Japanese row is a word of its own, and a key with no row keeps English. Setting the
    /// language is thread-local (`ui::set_current`), so this cannot race another test.
    #[test]
    fn japanese_rows_are_words_and_unknown_keys_keep_english() {
        super::super::ui::set_current("ja");
        for (english, japanese) in JAPANESE {
            assert_eq!(noun(english), *japanese, "{english}");
            assert!(!japanese.is_empty() && japanese != english, "{english}: {japanese}");
        }
        assert_eq!(noun("Nothing"), "Nothing");
        super::super::ui::set_current("en");
    }
}
