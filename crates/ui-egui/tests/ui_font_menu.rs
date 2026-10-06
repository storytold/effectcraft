//! #100: the Character panel's font menu lists the installed system fonts, not only the three
//! bundled families. Its own test binary, so nothing (theme install, a text layer resolving a
//! family) has scanned the system fonts before the panel asks for its list.

use effectcraft_engine::prefs::Prefs;
use effectcraft_engine::text::fonts;

#[test]
fn character_panel_font_menu_includes_system_fonts() {
    let installed = fonts::FontSource::faces(&fonts::DirectorySource::system());
    assert!(!fonts::system_scanned(), "nothing has scanned yet");
    // What the Character panel's font popup (and the Properties panel's font dropdown) shows.
    let rows = effectcraft_engine::font_menu(&Prefs::default());
    let families = effectcraft_engine::text_families();
    let bundled = ["Inter", "JetBrains Mono", "Noto Serif"];
    let Some(sys) = installed.iter().find(|f| !bundled.contains(&f.family.as_str())) else {
        assert!(!cfg!(target_os = "macos"), "macOS always has system fonts");
        eprintln!("no system fonts installed; skipping");
        return;
    };
    assert!(rows.iter().any(|r| r.family == sys.family), "{} missing from the font menu ({} rows)", sys.family, rows.len());
    assert!(families.contains(&sys.family));
    if cfg!(target_os = "macos") {
        assert!(rows.iter().any(|r| r.family == "Helvetica"), "Helvetica");
    }
}
