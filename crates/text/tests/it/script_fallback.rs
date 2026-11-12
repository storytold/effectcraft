//! Font fallback for characters of scripts with usual families gives the same face whatever earlier
//! lookups read. Its own test binary, so no installed font has been read when it starts.

use effectcraft_text::{fonts, variable};

#[test]
fn script_fallback_does_not_depend_on_earlier_lookups() {
    fonts::scan_system();
    let inter = fonts::resolve("Inter", "Regular").face;
    let scripts = ['א', 'م', 'क', 'த', 'ก', 'あ', '水', '한'];
    let before: Vec<_> = scripts.iter().map(|&c| fonts::fallback_for(c, inter)).collect();
    // A character that no usual family covers keeps its face after a character of the same block
    // that no face covers. On macOS only faces outside the Arabic list (.SF Arabic and .SF Arabic
    // Rounded) cover U+08A4, and no face covers U+0870.
    let first = fonts::fallback_for('\u{8a4}', inter);
    fonts::fallback_for('\u{870}', inter);
    let again = fonts::fallback_for('\u{8a4}', inter);
    assert_eq!(again, first, "U+08A4: {} → {}", fonts::face(first).info.family, fonts::face(again).info.family);
    // U+0590 is unassigned, in the Hebrew range. Armenian and Lao have no usual family; they share
    // 256-character blocks with Hebrew and Thai. The variable font search reads installed faces in
    // scan order. On macOS the first face in the scan, Arial Unicode MS, covers Armenian, Lao and
    // the eight characters in `scripts`.
    for c in ['\u{590}', 'Ա', 'ກ'] {
        fonts::fallback_for(c, inter);
    }
    variable::find_variable_face();
    let changed: Vec<String> = scripts
        .iter()
        .zip(&before)
        .filter_map(|(&c, &id)| {
            let after = fonts::fallback_for(c, inter);
            (after != id).then(|| format!("{c}: {} → {}", fonts::face(id).info.family, fonts::face(after).info.family))
        })
        .collect();
    assert!(changed.is_empty(), "{changed:?}");
}
