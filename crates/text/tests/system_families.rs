//! #100: the font list includes the system fonts without anything having scanned for them first.
//! Its own test binary, so no other test has triggered the system scan in this process.

use effectcraft_text::fonts;

#[test]
fn families_include_system_fonts_on_first_call() {
    // What the scan would find, read directly (not added to the database).
    let installed = fonts::FontSource::faces(&fonts::DirectorySource::system());
    assert!(!fonts::system_scanned(), "nothing has scanned yet");
    let fams = fonts::families();
    assert!(fonts::system_scanned(), "listing the families scans the system fonts");
    let Some(sys) = installed.iter().find(|f| !["Inter", "JetBrains Mono", "Noto Serif"].contains(&f.family.as_str())) else {
        assert!(!cfg!(target_os = "macos"), "macOS always has system fonts");
        eprintln!("no system fonts installed; skipping");
        return;
    };
    assert!(fams.iter().any(|(f, _)| *f == sys.family), "{} missing from {} families", sys.family, fams.len());
    if cfg!(target_os = "macos") {
        assert!(fams.iter().any(|(f, _)| f == "Helvetica"), "Helvetica");
    }
}
